# Conditions and routers

Conditions and routers are deterministic control-flow nodes. They use the
constrained template evaluator and cannot execute code.

## Conditions

`when` accepts one path, optional `not`, or equality against a JSON value:

```yaml
vars:
  enabled: true
when: "${{ vars.enabled == true }}"
```

Equality is type-sensitive. Missing paths fail instead of becoming false.
Explicit `null` is a value and is false when used directly. A false condition
sets the task to `skipped` with this durable output shape:

```json
{
  "reason": "when condition was false",
  "condition": {
    "expression": "${{ vars.enabled == true }}",
    "contextDigest": "sha256:v1:...",
    "result": false
  }
}
```

For a true condition, the same decision is retained in the transition audit
before the normal task output replaces the temporary decision value.

## Routers

A router is a pure task with an exact typed selector and enumerated
destinations:

```yaml
- id: route
  uses: router
  needs: [classify]
  route:
    select: "${{ tasks.classify.output.route }}"
    cases:
      - equals: approve
        tasks: [approved]
      - equals: reject
        tasks: [rejected]
    default: [manual]
```

Every destination must depend directly on the router. A destination may appear
only once. Duplicate typed case values, unknown destinations, implicit
dependencies, interpolated selectors, and missing cases fail compilation.
Case comparison uses JSON identity, so the number `1` and string `"1"` are
different.

The router output is:

```json
{
  "selected": "approve",
  "matched": true,
  "destinations": ["approved"]
}
```

Unselected destinations become `skipped` with the router ID and selected value
in their durable output. Their normal output contract is not evaluated because
they did not execute. A downstream task whose dependency was skipped is also
skipped.

## Recovery

The selector's dependency outputs and task variables participate in the
resolved-input digest. Changing an upstream classification invalidates the
router and its guarded descendants during repair. Terminal retry can restart a
router explicitly, and recorded replay copies route and skip decisions without
dispatching effects.

See [`examples/v1/router.yaml`](../../examples/v1/router.yaml).
