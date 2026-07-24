# Matrix and foreach tasks

`foreach` and `matrix` expand a task into a bounded set of ordinary compiled
tasks. Expansion happens during compilation, so model output cannot create an
unbounded graph.

## Foreach

```yaml
tasks:
  - id: inspect
    uses: agent:inspector
    foreach:
      items: [api, worker]
      as: service
      maxItems: 2
    with:
      prompt: "Inspect ${{ vars.service }}"
```

Each child receives the typed item as `vars.service` and its zero-based
position as `vars.foreachIndex`.

## Matrix

```yaml
tasks:
  - id: verify
    uses: action:assign
    matrix:
      axes:
        platform: [linux, macos]
        profile: [debug, release]
      maxItems: 4
    with:
      platform: "${{ vars.matrix.platform }}"
      profile: "${{ vars.matrix.profile }}"
      index: "${{ vars.matrixIndex }}"
```

Axes are traversed by axis name, then in each declared value order. `maxItems`
defaults to 32. The Cartesian product must fit that bound and the framework
maximum of 256. An empty axis produces an empty aggregate.

## Identity and aggregation

Compiled child IDs have the form
`PARENT--INDEX-BINDING_DIGEST`. The parent ID remains present as a pure
aggregate task. Its output is:

```json
{
  "items": [
    {
      "index": 0,
      "taskId": "verify--0000-...",
      "state": "succeeded",
      "output": {},
      "error": null
    }
  ]
}
```

Downstream tasks continue to depend on the parent ID and read
`tasks.PARENT.output.items`. Child IDs and bindings are visible in
`agentctl plan` and `agentctl inspect`.

## Failure and recovery

The expanded task's `failure` setting applies to every child. With `stop`, a
failed child stops the run. With `continue`, remaining children run and the
aggregate records every child state, output, and error; the run still finishes
failed because at least one task failed.

Retry and repair operate on child IDs. A failed-only retry reruns only failed
children and their aggregate while reusing compatible successful siblings.
Recorded replay reuses the aggregate and child outputs without provider or
tool effects.

Expansion bindings cannot shadow task variables. `foreach` and `matrix` are
mutually exclusive on a task. Runtime expansion from model-controlled arrays
is intentionally rejected; author a static bounded list or matrix instead.
