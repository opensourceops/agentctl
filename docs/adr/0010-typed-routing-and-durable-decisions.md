# ADR 0010: typed routing and durable decisions

Status: accepted

## Decision

Conditions remain a constrained path, typed comparison, and `not` language. Router
tasks are pure compiled nodes with one exact typed selector, unique JSON case
values, enumerated destination tasks, and optional default destinations.
Every destination declares the router as a dependency.

Condition transitions retain the expression, result, and evaluated-context
digest. Router output retains the selected JSON value, whether a case matched,
and the chosen destination IDs. Unselected destinations become skipped with an
explicit route decision.

## Consequences

- Arbitrary code and hidden model-owned routing remain impossible.
- JSON case comparison is type-sensitive.
- Changed selector dependencies invalidate router reuse during repair.
- Skipped tasks have a control-flow output contract separate from their normal
  execution output contract.
- Retry and recorded replay preserve the same visible decision boundaries.
