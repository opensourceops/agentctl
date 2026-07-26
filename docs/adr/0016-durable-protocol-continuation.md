# ADR 0016: Durable protocol continuation

## Status

Accepted.

## Context

MCP sessions can disappear while a workflow is running. A2A tasks can outlive
the connection that submitted or observed them. Blindly repeating either
operation can duplicate external mutation, while refusing every recovery makes
safe reads and known remote tasks unnecessarily brittle.

The runtime already persists effect identity and treats ambiguous delivery as
uncertain. Protocol recovery must refine that model without introducing a
second scheduler or claiming exactly-once delivery.

## Decision

SQLite schema 13 records protocol sessions and calls separately from the
generic effect ledger. Records include the run, task, attempt, effect, protocol,
operation, immutable call identity, generation, declared idempotency, status,
protected protocol state, and replay lineage.

MCP may reconnect once only when the workflow declares the call `pure`,
`idempotent`, or `keyed`. Reconnection creates a new session, refreshes the
tool catalog, and requires the selected tool schema digest to remain identical
before redispatch. `unknown` and `at_most_once` calls stop as uncertain.

A2A sends `SendMessage` once. When a response supplies a task ID, the runtime
persists it before polling or streaming. Observation may refresh a same-origin
Agent Card once and continue through `GetTask` or `SubscribeToTask`.
`effects continue-remote` observes that known task and records an applied
reconciliation. If submission was ambiguous before a task ID arrived, the
runtime refuses automatic continuation and never sends a second message.

Completed A2A artifacts are bounded, validated as typed parts, restricted to
same-origin URLs, and ingested into the content-addressed artifact store.
Repair or retry can materialize a schema-valid continued boundary and execute
only descendants. Replay copies protocol evidence with source linkage and
per-run recorded identities, but performs no network action.

Protocol SSE frames are accepted incrementally. Each bounded, redacted progress
record is persisted before the next frame is consumed. Final workflow output
still requires a complete validated protocol result.

## Consequences

- Workflow authors must make an honest MCP idempotency declaration to receive
  post-dispatch reconnect behavior.
- A changed MCP tool schema prevents redispatch even for an idempotent call.
- A2A continuation requires a known persisted task ID.
- Applied A2A continuation results are validated against the task output
  contract before they become repair or retry material.
- Protocol state can contain untrusted or sensitive remote data, so its JSON
  columns participate in selected-field envelope encryption and key rotation.
- The runtime provides at-most-once submission behavior, not exactly-once
  remote execution.

## Verification

- `cargo xtask protocol-resilience`
- `cargo xtask acceptance`, scenario 41
- retained-schema migration and encryption tests
- runtime continuation, retry materialization, artifact, and replay tests

The implementation follows the official [MCP Streamable HTTP
transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
[MCP tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools),
and [A2A 1.0 task lifecycle](https://a2a-protocol.org/latest/specification/).
