# Bounded loops

A task-level `loop` compiles into a fixed sequence of ordinary durable
iteration tasks and one pure aggregate. The graph exists before execution, so
neither a model nor runtime data can create an unbounded number of tasks.

```yaml
tasks:
  - id: refine
    uses: agent:reviewer
    loop:
      maxIterations: 3
      while: "${{ vars.loopIndex < 2 }}"
      initial:
        status: new
    with:
      prompt: "Iteration ${{ vars.loopIndex }}"
      previous: "${{ vars.loopPrevious }}"
```

`maxIterations` is required and accepts 1 through 64. `while` is an exact
typed condition. It is evaluated before each iteration. Numeric ordering
supports `<`, `<=`, `>`, and `>=`; equality and inequality use `==` and `!=`.
Ordering compares numbers only.

The body receives:

- `vars.loopIndex`: the zero-based iteration number.
- `vars.loopPrevious`: `initial` for iteration zero, then the complete output
  of the preceding iteration.

The condition may use `loop.output` as an authoring alias for
`vars.loopPrevious`, including nested paths. A task cannot combine `loop` with
`when`, `foreach`, `matrix`, or `route`, and loop bindings cannot shadow
task-local variables.

## Durable identity and output

Iteration IDs have the same stable
`PARENT--INDEX-BINDING_DIGEST` form as static expansion. Each iteration keeps
its own attempts, condition decision, output, error, effects, artifacts, and
audit history. `agentctl plan` exposes every ID and binding.

When the guard becomes false, that iteration and all remaining precompiled
iterations become `skipped`. The parent aggregate still succeeds and returns:

```json
{
  "iterations": 2,
  "items": [
    {
      "index": 0,
      "taskId": "refine--0000-...",
      "state": "succeeded",
      "output": {},
      "error": null
    }
  ]
}
```

`iterations` counts attempted, non-skipped iterations. If the condition remains
true after the final allowed iteration, the aggregate fails closed with a
maximum-iteration error.

## Recovery and effects

Retry and repair select iteration IDs, so a compatible prefix can be reused
while the selected boundary and its descendants execute again. Reused
iterations dispatch no providers, tools, processes, or network calls. Fresh
iterations use the normal effect ledger, idempotency, approval, and uncertain
effect reconciliation rules. Recorded replay copies iteration and aggregate
results without executing effects.

Cancellation marks every unfinished iteration and the aggregate cancelled.
The loop body's `failure` and retry settings apply independently to each
iteration.

See [`examples/v1/loop.yaml`](../../examples/v1/loop.yaml).
