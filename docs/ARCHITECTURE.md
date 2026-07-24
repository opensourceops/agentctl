# Architecture

## Dependency shape

```text
CLI ───────┬────────> runtime ─────> core
           │             │           ▲
           ├────────> providers ──────┤
           ├────────> protocols ─> runtime/core
           └────────> store ─────────>┘
runtime ────────────> observability ─> core contracts
```

`agentctl-core` owns deterministic domain behavior: strict parsing, migration, compilation, template resolution, effect identities, state machines, policy, and provider/tool interfaces. It knows no HTTP client, database, or CLI type. `agentctl-store` is the SQLite implementation. `agentctl-runtime` schedules stable bounded ready batches and coordinates injected clocks, IDs, executors, providers, protocols, persistence, and traces. Concrete network adapters and rendering stay at the edges.

## Execution

Parsing produces a versioned `Workflow`; compilation resolves references, validates provider capabilities and templates, detects cycles, and emits declaration-order topological tasks plus a digest. The runtime creates durable run/task rows, advances only valid state transitions, evaluates conditions, renders inputs, and executes the selected action or bounded agent.

An effect request is persisted before any filesystem observation/mutation, process, internal-memory update, long-term-memory operation, tool, model, MCP, or A2A call. Its stable identity covers run, task, task attempt, ordinal, operation, and input digest. A confirmed result can be reused on resume. A started but unconfirmed effect is uncertain and stops recovery rather than being repeated.

State transitions, checkpoint creation, working-memory replacement, artifact references, and audit insertion are transactionally coupled where consistency requires it. Artifact bytes live in an immutable SHA-256 content-addressed store beside SQLite; durable leases and a cross-process lock coordinate ingestion with reachability GC. Provider continuation, function-call correlation, effects, approvals, artifacts, and redacted trace events are inspectable through the public CLI. Long-term memory is a separate table and never participates in replay correctness. OpenTelemetry export remains optional and is not the audit log.

## Determinism and concurrency

Ready tasks are ordered by YAML declaration order after dependencies.
`maxConcurrency` defaults to one and is bounded at 64. A parallel batch reads
per-task durable memory snapshots, executes independently, then commits
successful outputs, disjoint memory deltas, failures, artifact references,
audit events, and the checkpoint in compiled order in one transaction.
Unordered overlapping `memoryWrites` fail compilation. Effects and provider
sessions remain task-local. See ADR 0008 and
[Deterministic parallel tasks](guides/PARALLEL_TASKS.md).

Loops, matrix/foreach expansion, routers, sub-workflows, handlers,
compensation execution, and event triggers still require their own explicit
state and recovery contracts. The DSL carries optional compensation metadata
on a tool contract, but the runtime does not execute compensation.

Clock and identifier generation are injected. Provider responses, tools, and external actions are injected interfaces. Cryptographic digests canonicalize identity; output maps use stable ordering where the public contract requires it.

## Platform and packaging

The workspace uses Rust edition 2024, pins Rust 1.88 as the MSRV, forbids unsafe code, and denies clippy warnings. HTTP uses rustls and disables redirects. Subprocesses use direct argv, a cleared environment, explicit allowlists, validated timeout/output limits, concurrent bounded pipe draining, cancellation, and kill/reap cleanup. SQLite is bundled for predictable installation and creates private files on Unix. SIGINT and SIGTERM converge on durable cancellation.

The OCI build is multi-stage: only the optimized Rust binary enters a maintained distroless runtime with CA roots and a non-root identity. `/config` is workflow configuration, `/workspace` is the read-only working tree, `/state` holds SQLite and the content-addressed artifact store, and `/artifacts` receives declared workflow outputs. State must be mounted again for inspect/resume/replay/repair and artifact export. The root filesystem may be read-only. See [Container contract](CONTAINER.md) and ADR 0007.

See the [architecture diagrams](architecture/DIAGRAMS.md), [ADRs](adr/), and [Durable execution](DURABLE_EXECUTION.md) for failure semantics.
