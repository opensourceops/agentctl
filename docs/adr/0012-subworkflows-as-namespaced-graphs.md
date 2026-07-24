# ADR 0012: sub-workflows as namespaced graphs

Status: accepted

## Decision

Reusable sub-workflows compile into the invoking plan. Each invocation creates
one typed input boundary, namespaced ordinary child tasks, and one typed output
aggregate. Definitions carry a semantic version and JSON Schemas for their
input and output interfaces. Pack definitions are covered by the existing
version and integrity pin.

The invoking workflow supplies policy and providers. Definitions cannot widen
policy. Deterministic memory state is invocation-prefixed, while effects,
artifacts, attempts, approvals, audit records, and traces remain owned by the
expanded child task.

## Consequences

- Scheduling, cancellation, failure propagation, retry, repair, replay, and
  inspection use the existing durable task model.
- Nested definitions are flattened recursively and cycles fail compilation.
- No second runtime, database, effect ledger, or hidden provider session exists.
- Namespaced IDs are part of recovery and inspection contracts.
- Dynamic memory keys are rejected because compile-time isolation cannot be
  proven.
