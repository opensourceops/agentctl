# Reusable sub-workflows

Sub-workflows package a typed graph behind one invocation task. Compilation
expands the graph into ordinary namespaced tasks, a durable typed input
boundary, and a typed output aggregate. There is no hidden nested scheduler.

```yaml
subworkflows:
  summarize:
    version: 1.0.0
    inputSchema:
      type: object
      required: [message]
      additionalProperties: false
      properties:
        message: { type: string }
    outputSchema:
      type: object
      required: [result]
      additionalProperties: false
      properties:
        result: { type: string }
    outputs:
      result: "${{ tasks.finish.output.output.result }}"
    tasks:
      - id: finish
        uses: action:assign
        with:
          result: "${{ inputs.message }}"

tasks:
  - id: summary
    uses: workflow:summarize
    with:
      message: durable
```

`version` must be semantic versioning. The invocation input is rendered once,
validated against `inputSchema`, and stored as a normal task output. References
to the definition's `inputs` read that boundary. The output map is rendered
after the child graph and validated against `outputSchema`.

Compiled IDs use `INVOCATION--LOCAL_TASK`. Input boundaries use a stable
digest-qualified `INVOCATION--inputs-DIGEST` ID. Nested sub-workflows expand
recursively. Cycles, missing local dependencies, invalid schemas, and ID
collisions fail compilation.

## Inheritance and isolation

The invoking workflow's policy is authoritative. A sub-workflow cannot add
policy grants. Providers are resolved from the invoking workflow, while
pack-provided agents, actions, tools, and sub-workflows receive integrity-pinned
pack-qualified names.

Working-memory keys and long-term-memory namespaces used by deterministic
memory actions are prefixed per invocation. Dynamic memory keys are rejected
because they cannot prove isolation before execution. Child effects, artifacts,
approvals, traces, and audit records retain their namespaced child task owner.

## Recovery

Every expanded child is an ordinary durable task. Retry and repair select the
visible namespaced IDs and reuse compatible predecessors. Failures report the
namespaced boundary. Recorded replay copies input, child, and output boundary
records without dispatching providers, tools, processes, or network calls.

Pack manifests may export definitions under `workflows`. A workflow pins the
pack name, version, path, and integrity digest, then invokes
`workflow:PACK_NAME.DEFINITION`.

See [`examples/v1/subworkflow.yaml`](../../examples/v1/subworkflow.yaml) and
[`examples/v1/reusable-pack.yaml`](../../examples/v1/reusable-pack.yaml).
