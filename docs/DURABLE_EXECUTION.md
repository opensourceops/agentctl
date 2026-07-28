# Durable execution

SQLite is the local history and correctness boundary. Run, task, effect, approval, checkpoint, audit, provider-session, tool-call, and long-term-memory records are schema-versioned. Future database, runtime, plan, effect, or checkpoint versions fail explicitly instead of being ignored.

Each run also owns a durable resource ledger. Known provider, tool, process,
and artifact units are atomically reserved before fresh dispatch, then replaced
with actual usage. Parallel tasks share the same SQLite coordinator. The
wall-time deadline is derived from run creation, so pause and resume cannot
reset it. Recorded replay and reused retry or repair boundaries dispatch
nothing and consume no fresh effect units. See [Resource and cost
budgets](guides/RESOURCE_BUDGETS.md).

## Operations

- Resume continues the same run from durable task state. Confirmed effects are reused. A requested-but-not-started effect may execute; a started-but-unconfirmed effect fails as uncertain.
- Recorded replay creates a replay record from terminal stored outputs and calls no provider, tool, network, process, or filesystem executor.
- Terminal retry creates a new source-linked run for an identical workflow, materializes compatible successful boundaries, and executes failed or explicitly selected roots plus their descendants with fresh attempts.
- Selective repair creates a new source-linked run, materializes compatible successful task outputs and committed state deltas, then executes selected roots and descendants with fresh effects from a target workflow.
- Compensation creates a source-linked sequential run for explicitly declared inverse actions. Confirmed inverse effects append `compensated` reconciliations to immutable source effects; partial failures remain independently retryable.
- Fork creates a new run linked to the old run and intentionally permits fresh effects.
- A task's `retry` policy creates another attempt inside the same run only within its explicit bound. An unsafe unresolved effect is not retried.

An effect ID is SHA-256 over run ID, task ID, task attempt, ordinal, operation, and input digest. Each record carries its format version, idempotency key, effect class, risk, status, request/result or error, timestamps, trace correlation, and confirmation flag. The request commits before the executor starts. This supports deterministic reuse of completed results but does not prove exactly-once behavior in an external system.

Pure operations need no external guarantee. Idempotent and keyed effects may be safely retried only when their implementation contract says so. Model calls and unknown remote mutations are treated at-most-once after start: a crash in the acknowledgement window creates an uncertain effect requiring operator reconciliation. Reconciliation appends an immutable `applied`, `not_applied`, or `compensated` conclusion with evidence; it does not rewrite the source effect. This is deliberately more conservative than silent at-least-once replay.

Working-memory replacement, the task transition, checkpoint, and audit event commit in one SQLite transaction. Tool-effect and tool-call terminal status also commit together, so inspection cannot observe one as completed while the other remains started. On resume, a confirmed memory-write effect is applied to the reconstructed working-memory value during the succeeding transition. Long-term reads and retrievals are recorded observation effects; writes are recorded external mutations; explicit promotion is an internal-state effect. Replay reuses recorded retrieval output without calling the memory or embedding provider. Repair re-executes a selected retrieval boundary and can observe newer entries. Long-term memory is not rolled back by replay.

Successful workspace mutations are ingested into the local content-addressed artifact store before task completion. Ingestion uses an atomic temporary file, SHA-256 identity, immutable deduplicated blobs, a cross-process lock, and a durable one-hour lease. Successful task completion then commits the artifact references with the definition fingerprint, resolved-input digest, output-contract fingerprint, output digest, immutable state delta and digest, audit event, and checkpoint, and releases the ingestion lease in the same SQLite transaction. Repair initialization starts from target initial memory and applies only reused successful task deltas in topological order. It never copies a terminal source's final memory snapshot.

Retry and repair planning are effect-free. A source task is reusable only when its metadata version, definition, dependencies, resolved inputs, output contract/value, state delta, content-addressed artifacts, and effect certainty are compatible. Retry additionally requires the exact stored workflow digest; changed definitions require repair. The new run stores the reused result, artifact references, and provenance in its own rows, so later source-row or workspace deletion does not break it. Missing or corrupt CAS bytes block reuse before a run is created.

Cancellation is both an injected token and a durable run flag. CLI SIGINT and SIGTERM cancel in-flight async calls and return exit `130`; `agentctl cancel` records a request for another process to observe. An overall CLI deadline can be set with `--timeout-seconds`, in addition to task/tool/provider/protocol bounds. A provider, tool, process, MCP, or A2A timeout/cancellation/transport loss after dispatch marks the effect `uncertain`; resume refuses to guess and requires reconciliation. An applied reconciliation supplies a validated recorded result. A not-applied or compensated reconciliation resumes with a fresh task and effect attempt.

A repaired agent task starts a fresh provider session. Source
`previous_response_id`, stateless continuation items, incomplete turns, pending
tool calls, and reasoning state are not copied. Validated task output and
reconstructed memory are the only cross-task/cross-run dataflow.

Clock and ID generation are injected; test providers/tools/protocol handlers
are injected. Ready tasks execute in bounded stable batches. Each reads a
persisted immutable memory snapshot, while task output, disjoint memory deltas,
artifacts, failures, audit events, and the checkpoint commit atomically in
compiled order.

Static foreach and matrix declarations compile before a run is created. Every
expanded child is a normal durable task with its own attempts, effects,
fingerprint, output, retry/repair identity, and replay record. The parent is a
pure aggregate task that records child IDs, states, outputs, and errors in
stable expansion order.

Condition transitions retain the expression, boolean result, and a canonical
digest of the evaluated inputs, variables, memory, and dependency outputs.
Pure router tasks retain their typed selected value and enumerated destination
IDs. Skipped branch records are copied directly by recorded replay and never
pass through a running state.

Bounded loops compile into a fixed sequential chain of ordinary tasks plus a
pure aggregate. Each iteration retains its guard decision, output, effects,
artifacts, attempts, and recovery identity. A false guard skips the remaining
chain. A guard that remains true after the declared maximum fails closed.

Sub-workflows also compile before run creation. Typed input and output boundary
tasks surround namespaced child tasks, so child attempts, effects, artifacts,
approvals, retry/repair lineage, cancellation, and replay stay in the ordinary
run graph.

Compensation planning is effect-free. Only confirmed successful mutations or
effects reconciled as applied are eligible. Started and uncertain effects
require operator reconciliation. The generated compensation run uses reverse
compiled order, ordinary effect identities, policy, approvals, retries,
checkpoints, audit, and traces. A repeated plan excludes source effects already
reconciled as compensated. This is best-effort inverse execution, not
transactional rollback.

The artifact root is `artifacts/` beside the database. `agentctl artifacts` lists references and blobs, verifies hashes, exports bytes atomically, and performs reachability-based collection. GC excludes referenced blobs and active ingestion leases, recovers interrupted quarantine operations on startup, and cleans stale untracked blobs and partial temporary files.
