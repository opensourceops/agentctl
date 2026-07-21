# Memory Model

This document defines the memory model for `agentctl` and the intended operational behavior of each memory mode.

## Overview

`agentctl` separates runtime durability from cross-run knowledge.

That split is deliberate:

- replay/resume correctness depends on local checkpointed state
- cross-run knowledge has different retention, query, and policy needs
- provider prompt caching is optimization, not correctness

The framework currently uses four memory modes conceptually:

1. `run_memory`
2. `working_memory`
3. `long_term_memory`
4. `prompt_cache`

## 1. Run Memory

Run memory is the execution state for one run.

It includes:

- playbook inputs
- task states
- task attempts
- agent sessions and turns
- checkpoints
- audit events
- trace spans
- the current working-memory snapshot

Storage:

- runtime DB
- default path: `~/.agentctl/runtime/runtime.db`

Requirements:

- deterministic
- replay-safe
- resume-safe
- local-first

Run memory should not depend on external stores.

## 2. Working Memory

Working memory is the mutable state for the active run.

It is intended for:

- facts discovered during execution
- intermediate conclusions
- handoff state between tasks or agents
- structured scratch state that must survive retries and resume

Storage:

- checkpointed inside the runtime DB as part of the run snapshot

Current template surface:

- canonical: `memory.working.*`
- compatibility mirror: `vars.*`

Example:

```yaml
memory:
  working:
    initial:
      service: checkout
```

And later:

```yaml
tasks:
  - id: remember
    uses: module:builtin.memory.write
    with:
      key: finding
      value: restore-drill-missing
```

## 3. Long-Term Memory

Long-term memory is cross-run durable knowledge.

Use it for:

- approved facts
- reusable operational knowledge
- indexed reports
- retained findings that should survive independent runs

Do not use it for:

- transient per-run scratch state
- checkpoint/replay correctness
- provider cache material

Current implementation:

- local SQLite store
- default path: `~/.agentctl/memory/long-term.db`
- adapter extension point scaffolded under `src/long-term-memory-adapters/`

Current access paths:

- playbook modules
  - `builtin.long_term_memory.write`
  - `builtin.long_term_memory.search`
- CLI
  - `agentctl memory get`
  - `agentctl memory search`
  - `agentctl memory write`
  - `agentctl memory stats`

### Namespace model

Long-term memory is namespaced.

That prevents unrelated playbooks or environments from writing into the same logical key space by accident.

If a playbook omits a namespace, compilation defaults it to the playbook name.

For the CLI:

- `memory write` defaults to namespace `default` if `--namespace` is omitted
- `memory get` and `memory search` work across all namespaces when `--namespace` is omitted
- `memory stats` reports all namespaces when `--namespace` is omitted

### Why external adapters belong here

If `agentctl` later connects to external SQL, document, vector, or graph backends, `long_term_memory` is the correct integration point.

Reason:

- cross-run retrieval belongs here
- semantic search belongs here
- retention/governance belongs here
- runtime correctness does not depend on it

### Adapter extension surface

`agentctl` now includes a placeholder adapter surface for future built-in and community backends.

Current adapter files:

- `sqlite`
- `postgres`
- `pgvector`
- `elasticsearch`
- `qdrant`
- `weaviate`
- `pinecone`
- `document`
- `graph`

Only `sqlite` is implemented today.

The others are placeholders with a stable contract:

- `write(namespace, key, value, tags?)`
- `get(namespace, key)`
- `search(namespace, query, key, limit)`
- `getStats(namespace?)`
- `close()`

Community adapters can implement the same interface and later be wired into runtime/CLI configuration without changing the core memory semantics.

## 4. Prompt Cache

Prompt cache is not a memory-of-record.

It is a provider-native optimization layer for:

- repeated prompt prefixes
- tool schema reuse
- lower repeated input-token cost
- lower repeated prompt latency

Current implementation:

- supported for `openai.responses` with provider `openai`
- disabled by default
- configured at playbook or agent level
- observed through runtime audit events and `agentctl prompt-cache stats`

Prompt cache must remain optional and disposable.

It should never be required for correctness.

## CLI Reference

### `agentctl memory stats`

Inspect the long-term memory DB.

```bash
agentctl memory stats
agentctl memory stats --namespace memory-flow
agentctl memory stats --output json
```

Output fields:

- `dbPath`
- `fileSizeBytes`
- `totalEntries`
- `totalNamespaces`
- `oldestCreatedAt`
- `newestUpdatedAt`
- optional filtered `namespace`
- `namespaces[]`
  - `namespace`
  - `entries`
  - `oldestCreatedAt`
  - `newestUpdatedAt`

### `agentctl memory get`

Exact-key lookup.

```bash
agentctl memory get finding
agentctl memory get finding --namespace memory-flow
```

Behavior:

- with `--namespace`, returns exact matches within that namespace
- without `--namespace`, returns exact-key matches across all namespaces

Output fields:

- `dbPath`
- `namespace` or `null`
- `key`
- `limit`
- `found`
- `matchCount`
- `matches[]`

### `agentctl memory search`

Search by text query or exact key.

```bash
agentctl memory search --query restore
agentctl memory search --namespace memory-flow --query readiness
agentctl memory search --key finding
```

Behavior:

- `--query` searches key, serialized value, and serialized tags
- `--key` performs exact-key matching
- if both are omitted, returns entries up to `--limit`

### `agentctl memory write`

Write a long-term memory entry.

```bash
agentctl memory write finding --namespace memory-flow --string restore-drill-missing
agentctl memory write finding --namespace memory-flow --value '{"status":"missing"}'
agentctl memory write finding --tags readiness,audit --string restore-drill-missing
```

Behavior:

- creates the DB if it does not exist
- requires exactly one of:
  - `--value` for JSON
  - `--string` for plain text
- tags are optional and comma-separated

## `vars` Decision

`vars` is currently retained as a compatibility mirror of `memory.working`.

This is the right short-term tradeoff because it avoids breaking:

- older playbooks
- older templates
- tests and examples that still reference `vars`

But the framework direction is:

- canonical state: `memory.working`
- compatibility-only mirror: `vars`

New playbooks should use `memory.working`.

Future work can de-emphasize `vars` in output and documentation before eventually removing it, but correctness should continue to rely on `memory.working`.
