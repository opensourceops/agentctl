# ADR 0009: bounded static task expansion

Status: accepted

## Decision

`foreach` item lists and matrix axes expand during compilation. Each child has
a stable parent, ordinal, binding digest, and ordinary durable task record.
The authored parent ID becomes a pure ordered aggregate of the child records.
Every declaration provides `maxItems`, and the framework rejects more than 256
children.

## Consequences

- Model output cannot grow the graph.
- Child retry, repair, replay, effects, policy, cancellation, and inspection
  reuse the existing task model.
- Downstream dependencies remain attached to the authored parent ID.
- `failure: continue` allows aggregation of partial results, but a run with a
  failed child still finishes failed.
- Changing an item or axis changes the affected child identity and workflow
  digest.
