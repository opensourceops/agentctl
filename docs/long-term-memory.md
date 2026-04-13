# Long-Term Memory Operations

This document covers the operational surface of `long_term_memory` in `agentctl`:

- retention and garbage collection
- adapter selection
- MongoDB Atlas support
- agent-facing retrieval and promotion patterns
- replay/resume behavior for memory-heavy agent flows

For the broader memory model, see [/Users/ompragash/Git/agentctl/docs/memory.md](/Users/ompragash/Git/agentctl/docs/memory.md).

## Scope

`long_term_memory` is the cross-run durable knowledge layer.

It is the correct place for:

- approved findings
- reusable facts
- retained operational context
- external memory backends such as SQL, vector, document, and graph stores

It is not the place for:

- per-run checkpoint correctness
- transient scratch state
- provider prompt cache material

## Retention and GC

`agentctl` now supports first-class garbage collection for long-term memory:

```bash
agentctl memory gc
agentctl memory gc --older-than-days 7 --keep-entries 50
agentctl memory gc --namespace service-audit --output json --verbose
```

Behavior:

- deletes entries older than the configured cutoff
- keeps the newest `N` entries even if they are older than the cutoff
- supports optional namespace scoping
- for SQLite, runs `VACUUM` after deletions
- for MongoDB Atlas, no vacuum step exists, so `vacuumed` remains `false`

Output fields:

- `provider`
- `olderThanDays`
- `keepEntries`
- `deletedEntries`
- `vacuumed`
- `before`
- `after`
- `deletedKeys` in verbose mode

## Adapter Selection

Current supported runtime providers:

- `sqlite`
- `mongodb-atlas`

Current scaffold-only placeholders:

- `postgres`
- `pgvector`
- `elasticsearch`
- `qdrant`
- `weaviate`
- `pinecone`
- `document`
- `graph`

Only `sqlite` and `mongodb-atlas` are functional today.

## SQLite

SQLite remains the local default.

Playbook config:

```yaml
memory:
  longTerm:
    provider: sqlite
    dbPath: ./state/long-term.db
    namespace: service-audit
```

CLI examples:

```bash
agentctl memory write finding --db ./state/long-term.db --namespace service-audit --string restore-drill-missing
agentctl memory get finding --db ./state/long-term.db --namespace service-audit
agentctl memory gc --db ./state/long-term.db --older-than-days 30 --keep-entries 100
```

## MongoDB Atlas

MongoDB Atlas is now supported as a real long-term memory adapter.

Playbook config:

```yaml
memory:
  longTerm:
    provider: mongodb-atlas
    connectionStringEnv: AGENTCTL_MONGODB_URI
    database: agentctl
    collection: long_term_memories
    namespace: service-audit
```

CLI examples:

```bash
agentctl memory write finding \
  --provider mongodb-atlas \
  --connection-string "$AGENTCTL_MONGODB_URI" \
  --database agentctl \
  --collection long_term_memories \
  --namespace service-audit \
  --string restore-drill-missing

agentctl memory search \
  --provider mongodb-atlas \
  --connection-string "$AGENTCTL_MONGODB_URI" \
  --database agentctl \
  --collection long_term_memories \
  --query restore
```

Implementation notes:

- connection string comes from `connectionString` or `connectionStringEnv`
- database default: `agentctl`
- collection default: `long_term_memories`
- indexes are created on:
  - `{ namespace: 1, key: 1 }` unique
  - `{ namespace: 1, updatedAt: -1 }`

## Agent-Facing Retrieval Patterns

The framework now supports a higher-level retrieval-and-promotion module:

- `builtin.long_term_memory.retrieve`

Purpose:

- search long-term memory
- select one or more results
- promote selected data into `working_memory`

This is more useful for agents than a raw search followed by a separate working-memory write.

### Inputs

- `namespace`
- `query`
- `key`
- `limit`
- `select`
  - `first`
  - `all`
- `promoteKey`
- `promoteMode`
  - `value`
  - `entry`
  - `matches`
  - `values`
- `includeMetadata`

### Behavior

- `select: first`
  - uses the first matching entry
- `select: all`
  - uses all matching entries
- `promoteKey`
  - required
  - target key in `memory.working`
- `promoteMode: value`
  - promotes the entry value
- `promoteMode: values`
  - promotes an array of values
- `promoteMode: entry`
  - promotes a single full entry
- `promoteMode: matches`
  - promotes the selected entry or entries with metadata

### Example

```yaml
tasks:
  - id: recall_incident_owner
    uses: module:builtin.long_term_memory.retrieve
    with:
      key: incident-owner
      promoteKey: recalled_owner
      select: first
      promoteMode: value
```

After this runs successfully:

- `memory.working.recalled_owner` is available to later tasks and agents
- `vars.recalled_owner` mirrors it for compatibility

## Replay and Resume Semantics

Memory-heavy agent flows are now covered by targeted regressions:

- resume after a mid-agent working-memory turn
- replay from a checkpoint inside a multi-turn memory agent

What is guaranteed:

- `working_memory` survives checkpoints
- agent session turns survive checkpoints
- replay from an agent checkpoint resumes from that stored agent/session state
- promoted working-memory values remain available after resume/replay

What is not guaranteed:

- `long_term_memory` side effects are not “rolled back”
- replaying from a checkpoint after an external long-term write continues from the stored checkpoint state, which is correct for durable side effects

## Community Extensions

The long-term adapter surface is intentionally separated under:

- [/Users/ompragash/Git/agentctl/src/long-term-memory-adapters/](/Users/ompragash/Git/agentctl/src/long-term-memory-adapters/)

The contract includes:

- `write`
- `get`
- `search`
- `getStats`
- `garbageCollect`
- `close`

That is the extension point for future built-in and community adapters.
