# Durable execution

SQLite is the local history and correctness boundary. Run, task, effect, approval, checkpoint, audit, provider-session, tool-call, and long-term-memory records are schema-versioned. Future database, runtime, plan, effect, or checkpoint versions fail explicitly instead of being ignored.

## Operations

- Resume continues the same run from durable task state. Confirmed effects are reused. A requested-but-not-started effect may execute; a started-but-unconfirmed effect fails as uncertain.
- Recorded replay creates a replay record from terminal stored outputs and calls no provider, tool, network, process, or filesystem executor.
- Fork creates a new run linked to the old run and intentionally permits fresh effects.
- Retry creates a new task attempt only within the task’s explicit bound. An unsafe unresolved effect is not retried.

An effect ID is SHA-256 over run ID, task ID, task attempt, ordinal, operation, and input digest. Each record carries its format version, idempotency key, effect class, risk, status, request/result or error, timestamps, trace correlation, and confirmation flag. The request commits before the executor starts. This supports deterministic reuse of completed results but does not prove exactly-once behavior in an external system.

Pure operations need no external guarantee. Idempotent and keyed effects may be safely retried only when their implementation contract says so. Model calls and unknown remote mutations are treated at-most-once after start: a crash in the acknowledgement window creates an uncertain effect requiring operator reconciliation or an explicit fork. This is deliberately more conservative than silent at-least-once replay.

Working-memory replacement, the task transition, checkpoint, and audit event commit in one SQLite transaction. Tool-effect and tool-call terminal status also commit together, so inspection cannot observe one as completed while the other remains started. On resume, a confirmed memory-write effect is applied to the reconstructed working-memory value during the succeeding transition. Long-term memory is an external effect and is not rolled back by replay.

Cancellation is both an injected token and a durable run flag. CLI SIGINT and SIGTERM cancel in-flight async calls and return exit `130`; `agentctl cancel` records a request for another process to observe. An overall CLI deadline can be set with `--timeout-seconds`, in addition to task/tool/provider/protocol bounds. A provider, tool, process, MCP, or A2A timeout/cancellation/transport loss after dispatch marks the effect `uncertain`; resume refuses to guess and requires reconciliation or an explicit fork.

Clock and ID generation are injected; test providers/tools/protocol handlers are injected. The current scheduler is sequential, so output and memory commit order is task declaration order.
