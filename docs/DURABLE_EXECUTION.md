# Durable execution

SQLite is the local history and correctness boundary. Run, task, effect, approval, checkpoint, audit, provider-session, tool-call, and long-term-memory records are schema-versioned. Future database, runtime, plan, effect, or checkpoint versions fail explicitly instead of being ignored.

## Operations

- Resume continues the same run from durable task state. Confirmed effects are reused. A requested-but-not-started effect may execute; a started-but-unconfirmed effect fails as uncertain.
- Recorded replay creates a replay record from terminal stored outputs and calls no provider, tool, network, process, or filesystem executor.
- Selective repair creates a new source-linked run, materializes compatible successful task outputs and committed state deltas, then executes selected roots and descendants with fresh effects from a target workflow.
- Fork creates a new run linked to the old run and intentionally permits fresh effects.
- Retry creates a new task attempt only within the task’s explicit bound. An unsafe unresolved effect is not retried.

An effect ID is SHA-256 over run ID, task ID, task attempt, ordinal, operation, and input digest. Each record carries its format version, idempotency key, effect class, risk, status, request/result or error, timestamps, trace correlation, and confirmation flag. The request commits before the executor starts. This supports deterministic reuse of completed results but does not prove exactly-once behavior in an external system.

Pure operations need no external guarantee. Idempotent and keyed effects may be safely retried only when their implementation contract says so. Model calls and unknown remote mutations are treated at-most-once after start: a crash in the acknowledgement window creates an uncertain effect requiring operator reconciliation. Reconciliation appends an immutable `applied`, `not_applied`, or `compensated` conclusion with evidence; it does not rewrite the source effect. This is deliberately more conservative than silent at-least-once replay.

Working-memory replacement, the task transition, checkpoint, and audit event commit in one SQLite transaction. Tool-effect and tool-call terminal status also commit together, so inspection cannot observe one as completed while the other remains started. On resume, a confirmed memory-write effect is applied to the reconstructed working-memory value during the succeeding transition. Long-term memory is an external effect and is not rolled back by replay.

Successful workspace mutations are ingested into the local content-addressed artifact store before task completion. Ingestion uses an atomic temporary file, SHA-256 identity, immutable deduplicated blobs, a cross-process lock, and a durable one-hour lease. Successful task completion then commits the artifact references with the definition fingerprint, resolved-input digest, output-contract fingerprint, output digest, immutable state delta and digest, audit event, and checkpoint, and releases the ingestion lease in the same SQLite transaction. Repair initialization starts from target initial memory and applies only reused successful task deltas in topological order. It never copies a terminal source's final memory snapshot.

Repair planning is effect-free. A source task is reusable only when its metadata version, definition, dependencies, resolved inputs, output contract/value, state delta, content-addressed artifacts, and effect certainty are compatible. The repair run stores the reused result, artifact references, and provenance in its own rows, so later source-row or workspace deletion does not break it. Missing or corrupt CAS bytes block reuse before a repair run is created.

Cancellation is both an injected token and a durable run flag. CLI SIGINT and SIGTERM cancel in-flight async calls and return exit `130`; `agentctl cancel` records a request for another process to observe. An overall CLI deadline can be set with `--timeout-seconds`, in addition to task/tool/provider/protocol bounds. A provider, tool, process, MCP, or A2A timeout/cancellation/transport loss after dispatch marks the effect `uncertain`; resume refuses to guess and requires reconciliation. An applied reconciliation supplies a validated recorded result. A not-applied or compensated reconciliation resumes with a fresh task and effect attempt.

A repaired agent task starts a fresh provider session. Source `previous_response_id`, incomplete turns, pending tool calls, and reasoning state are not copied. Validated task output and reconstructed memory are the only cross-task/cross-run dataflow.

Clock and ID generation are injected; test providers/tools/protocol handlers are injected. The current scheduler is sequential, so output and memory commit order is task declaration order.

The artifact root is `artifacts/` beside the database. `agentctl artifacts` lists references and blobs, verifies hashes, exports bytes atomically, and performs reachability-based collection. GC excludes referenced blobs and active ingestion leases, recovers interrupted quarantine operations on startup, and cleans stale untracked blobs and partial temporary files.
