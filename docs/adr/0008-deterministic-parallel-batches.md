# ADR 0008: Deterministic parallel batches

Status: accepted, 2026-07-24.

Independent ready tasks may execute concurrently when
`runtime.maxConcurrency` is greater than one. Selection follows compiled plan
order and never exceeds 64 tasks.

Each task receives a durable immutable working-memory snapshot. Completed
results commit in compiled order as one SQLite transaction, including task
states, working-memory deltas, artifact references, audit events, run failure
when applicable, and the checkpoint.

Working-memory write sets are part of the compiled task. Literal
`builtin.memory.write` keys are inferred, templated keys require
`memoryWrites`, and unordered overlaps fail compilation. No implicit merge
strategy exists.

This replaces the sequential-only scheduling decision in ADR 0005. The default
concurrency remains one for compatibility.
