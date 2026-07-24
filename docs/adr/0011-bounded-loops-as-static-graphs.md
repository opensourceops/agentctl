# ADR 0011: bounded loops as static graphs

Status: accepted

## Decision

A task-level loop compiles into a statically bounded chain of iteration tasks
and one pure aggregate. `maxIterations` is mandatory and capped at 64. The
typed `while` condition is evaluated before each iteration. A false condition
durably skips the remaining chain. A still-true condition after the final
iteration fails the aggregate.

Each iteration has a stable digest-qualified ID, a zero-based `loopIndex`, and
a typed `loopPrevious` binding containing either the declared initial value or
the preceding iteration's full output.

## Consequences

- The scheduler never accepts model-controlled graph growth.
- Existing task attempts, effects, approvals, artifacts, retry, repair,
  cancellation, and replay semantics apply at every iteration boundary.
- The compiled graph contains at most 64 iteration nodes per loop declaration.
- Loops are sequential. Independent graph branches remain the mechanism for
  deterministic parallel work.
- Authors cannot combine `loop` with another task expansion, router, or `when`.
