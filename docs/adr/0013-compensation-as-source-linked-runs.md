# ADR 0013: Compensation as source-linked runs

- Status: accepted
- Date: 2026-07-24

## Context

Applied external mutations sometimes need an explicit inverse operation.
Mutating a terminal source run would weaken run immutability, while pretending
that inverse operations form a transaction would overstate external-system
guarantees.

## Decision

Each compensable task declares a named effectful action and bounded execution
settings. The runtime plans from durable source effects, orders eligible tasks
in reverse compiled graph order, and executes them as a separate sequential
run with `mode: compensation` and `sourceRunId`.

Compensation uses ordinary tasks, policy, approvals, effect identities,
uncertainty handling, retries, cancellation, checkpoints, audit, and traces.
Successful compensation effects append `compensated` reconciliation records
to immutable source effects. A repeated plan excludes those effects.

Manual execution is the default. Automatic execution occurs only when
`compensation.onFailure` is `automatic`.

## Consequences

- The source run and source effects remain immutable.
- Partial compensation is durable and retryable without repeating completed
  inverse effects.
- An uncertain source or compensation effect requires reconciliation.
- Retry and repair cannot reuse a task whose applied effect was compensated.
- Compensation remains an honest best-effort operation, not rollback,
  exactly-once delivery, or a distributed transaction.
