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

State transitions, checkpoint creation, working-memory replacement, artifact references, and audit insertion are transactionally coupled where consistency requires it. A per-run SQLite budget ledger atomically coordinates parallel reservations before effect dispatch and actual-usage reconciliation afterward. Artifact bytes live in an immutable SHA-256 content-addressed store beside SQLite; durable leases and a cross-process lock coordinate ingestion with reachability GC. Provider continuation, function-call correlation, effects, approvals, artifacts, budgets, and redacted trace events are inspectable through the public CLI. Long-term memory is a separate table and never participates in replay correctness. OpenTelemetry export remains optional and is not the audit log.

## Determinism and concurrency

Ready tasks are ordered by YAML declaration order after dependencies.
`maxConcurrency` defaults to one and is bounded at 64. A parallel batch reads
per-task durable memory snapshots, executes independently, then commits
successful outputs, disjoint memory deltas, failures, artifact references,
audit events, and the checkpoint in compiled order in one transaction.
Unordered overlapping `memoryWrites` fail compilation. Effects and provider
sessions remain task-local. See ADR 0008 and
[Deterministic parallel tasks](guides/PARALLEL_TASKS.md).

Static foreach lists and matrix axes compile into ordinary namespaced child
tasks followed by a pure aggregate. Their IDs, bindings, attempts, outputs,
and recovery lineage use the same durable task model as authored nodes. Typed
routers are pure tasks whose enumerated destination guards compile into the
graph; condition and route decisions are durable and replayable. Bounded loops
compile into sequential namespaced iteration tasks and a pure aggregate, so
iteration attempts, effects, guard decisions, retry, repair, and replay use the
ordinary durable task model. Reusable sub-workflows compile into a typed input
boundary, namespaced ordinary tasks, and a typed output aggregate. Their policy
and providers come from the invoking workflow, while deterministic memory keys
are invocation-prefixed. Explicit compensation plans eligible applied effects
in reverse graph order and executes ordinary actions in a separate
source-linked run. Confirmed inverse effects append immutable reconciliation
records to the source. Structured handoffs are typed deterministic tasks
between bounded agent tasks, so role collaboration retains ordinary task-local
provider sessions, effects, audit, retry, repair, and replay. Streaming
providers pass typed fragments through an awaited runtime sink that bounds and
persists each task-attempt event before consuming more transport data. Final
result validation remains unchanged. Handlers and event triggers remain
outside the runtime surface.

Clock and identifier generation are injected. Provider responses, tools, and external actions are injected interfaces. Cryptographic digests canonicalize identity; output maps use stable ordering where the public contract requires it.

## Platform and packaging

The workspace uses Rust edition 2024, pins Rust 1.88 as the MSRV, forbids
unsafe code, and denies clippy warnings. HTTP uses rustls, ignores environment
proxies by default, disables redirects and Unix sockets, checks every resolved
address, and pins accepted direct DNS answers. Optional custom roots come only
from protected certificate-only PEM references. Host subprocesses use direct
argv, a cleared environment, explicit allowlists, validated timeout/output
limits, concurrent bounded pipe draining, cancellation, and kill/reap cleanup;
this `process` mode is not a sandbox. Explicit `container` actions use a local
digest-pinned Docker/Podman image with no pull/network, a read-only
root/workspace, a non-root identity, dropped capabilities, resource limits,
and fail-closed backend/image preflight. SQLite is bundled for predictable
installation and creates private files on Unix. SIGINT and SIGTERM converge on
durable cancellation. See [Process isolation](guides/PROCESS_ISOLATION.md) and
ADR 0020. Run budget coordination is described by ADR 0021.

The OCI build is multi-stage: only the optimized Rust binary enters a maintained distroless runtime with CA roots and a non-root identity. `/config` is workflow configuration, `/workspace` is the read-only working tree, `/state` holds SQLite and the content-addressed artifact store, and `/artifacts` receives declared workflow outputs. State must be mounted again for inspect/resume/replay/repair and artifact export. The root filesystem may be read-only. See [Container contract](CONTAINER.md) and ADR 0007.

See the [architecture diagrams](architecture/DIAGRAMS.md), [ADRs](adr/), and [Durable execution](DURABLE_EXECUTION.md) for failure semantics.
