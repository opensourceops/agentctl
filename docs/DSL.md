# Workflow DSL

The current document version is `agentctl.dev/v1alpha1`, with `kind: Workflow`. The generated, authoritative JSON Schema is [`schemas/workflow.schema.json`](../schemas/workflow.schema.json). YAML documents are limited to 1 MiB and reject unknown fields.

`metadata` contains the name, description, and labels. `spec` contains typed inputs/outputs; providers; bounded agents; actions; tool contracts; reusable sub-workflows; compensation policy; ordered tasks; policy; memory; MCP servers; A2A peers; packs; runtime; and output settings. A task `uses` `action:<name>`, `agent:<name>`, `workflow:<name>`, or the pure `router` construct. Tasks declare `needs`, optional bounded `foreach`, `matrix`, or `loop` expansion, optional working-memory `memoryWrites`, an optional `when`, local `vars`, typed `with` input, optional `outputSchema`, retry, timeout, failure behavior, and an optional effectful `compensate` action.

Templates use only `${{ inputs.path }}`, `${{ vars.path }}`, `${{ memory.path }}`, and `${{ tasks.task-id.output.path }}`. Conditions additionally allow `not` and equality against a JSON literal or string. Exact templates preserve their JSON type; interpolation into text accepts only scalars. Missing and explicit `null` are different. There is no code execution, function call, indexing, arithmetic, or implicit task dependency.

`when` decisions retain the expression, boolean result, and a digest of the
evaluated context in durable task/audit state. A `router` selects one exact
typed template, compares it with type-sensitive enumerated cases, and records
the selected value and explicit destinations. Unselected destinations are
durably skipped. See [Conditions and routers](guides/CONDITIONS_AND_ROUTERS.md).

Task output is JSON. Built-in actions own an object contract, agents can declare provider-enforced `structuredOutput`, and a task can override the complete contract with `outputSchema`. The compiler validates schemas; the runtime validates completed and selectively reused values.

Providers, action environments, and protocol headers use secret references:
`{ env: NAME }`, `{ file: PATH }`, or a bounded `{ process: ... }` reference.
File references require `policy.secretFileRoots`; process references require
`policy.secretProcessAllowlist`. Existing environment references remain
compatible. Resolved values never become the workflow document, effect value,
trace, or inspection output. See [Secret references](guides/SECRET_REFERENCES.md).

`policy.workspaceRoot` is the default boundary for relative file paths. Each `writableRoots` entry may be workspace-relative or an explicit absolute mount such as `/artifacts`. Ordinary reads remain workspace-confined. After a successful authorized mutation, the runtime may read that exact output through its writable-root boundary to ingest the bounded regular file into durable CAS; this does not grant tasks general read access to the external root.

`policy.networkAllowlist` grants exact hosts or `*.suffix` subdomains.
`policy.network` constrains `allowedSchemes`, `allowedPorts`,
`allowPrivate`, `allowProxy`, `customCa`, `connectTimeoutSeconds`, and
`maxResponseBytes`. Private addresses and proxies default to denied. Required
provider and protocol endpoints are resolved, checked, and pinned before run
state is created. See [Network policy](guides/NETWORK_POLICY.md).

The compiler validates missing references, duplicate tasks, cycles, task-aware templates, tool references, provider capabilities, agent limits, concurrency bounds, and working-memory conflicts before execution. `maxConcurrency` accepts `1` through `64` and defaults to `1`. Independent ready tasks are selected in compiled order, execute against durable isolated memory snapshots, and commit atomically in compiled order. Literal working-memory keys are inferred; templated keys require `memoryWrites`. See [Deterministic parallel tasks](guides/PARALLEL_TASKS.md).

Static `foreach` lists and matrix axes expand at compile time into stable child
tasks plus a parent aggregate. `maxItems` defaults to 32, expansion cannot
exceed 256 children, and model output cannot drive it. Retry and repair can
select the visible child IDs. See [Matrix and foreach tasks](guides/MATRIX_AND_FOREACH.md).

Bounded `loop` tasks require `maxIterations` from 1 through 64 and one exact
typed `while` guard. They compile into stable sequential iteration tasks.
`vars.loopIndex` is the zero-based position and `vars.loopPrevious` is the
initial value or preceding iteration output. A still-true guard after the
maximum fails closed. Retry and repair select iteration IDs. See [Bounded
loops](guides/BOUNDED_LOOPS.md).

Reusable `subworkflows` declare a semantic version, input/output JSON Schemas,
default input values, an output map, and local tasks. An invocation compiles to
a typed input boundary, `INVOCATION--LOCAL_TASK` children, and a typed output
aggregate. Pack manifests export the same contract under `workflows`. See
[Reusable sub-workflows](guides/SUB_WORKFLOWS.md).

Compensable tasks declare one named effectful action under `compensate`.
Compensation is manual unless `spec.compensation.onFailure` is `automatic`.
Planning uses confirmed source effects, runs inverse actions in reverse graph
order, and appends linked `compensated` reconciliation records. See
[Compensate applied effects](guides/COMPENSATION.md).

Structured role collaboration uses ordinary agent tasks, typed deterministic
handoff tasks, routers, and reusable sub-workflows. `uses: team:<name>` is
rejected because hidden conversation state would bypass the compiled graph.
See [Structured role handoffs](guides/STRUCTURED_HANDOFFS.md).

An agent may set `stream: true` when its provider advertises streaming. Stream
fragments are bounded, redacted, persisted under task-attempt sequence numbers,
and kept separate from final task output validation. See [Durable provider
streaming](guides/DURABLE_STREAMING.md).

`builtin.shell.exec` and `extension.process` expose `isolation: process` by
default. This is bounded host execution, not a sandbox. `isolation: container`
requires a local digest-pinned image plus Docker or Podman and runs with a
read-only root/workspace, no network, a non-root identity, dropped
capabilities, and explicit memory/CPU/PID limits. The requested engine and
image fail closed without host fallback. See [Process
isolation](guides/PROCESS_ISOLATION.md).

`runtime.budgets` sets optional run-wide request, turn, tool, token, wall-time,
process-output, artifact, task, expansion, loop, and monetary ceilings. Static
graph counts fail compilation. Dynamic units are reserved atomically before
fresh effect dispatch and reconciled from actual usage. Monetary limits use
integer micro-US-dollars and require versioned `runtime.pricing` entries keyed
by `provider/model`. See [Resource and cost
budgets](guides/RESOURCE_BUDGETS.md).

`builtin.shell.exec` captures stdout and stderr concurrently. Its optional `stdoutLimitBytes`, `stderrLimitBytes`, and `combinedOutputLimitBytes` fields default to 1 MiB, 1 MiB, and 2 MiB respectively. Each configured value must be between 1 byte and 16 MiB. `timeoutSeconds` must be between 1 and 86,400. Exceeding an output bound terminates and reaps the process and records a structured failed effect; timeout or cancellation remains an uncertain effect because external changes may already have occurred. These fields are validated identically for workflow and pack actions.

The parser translates a limited unversioned `playbook:` document and emits a migration warning. Use `agentctl migrate old.yaml --write new.yaml`. Legacy pack-backed, MCP, A2A, provider-specific, and broad module configurations need manual migration; see [Migrating from TypeScript](MIGRATING_FROM_TYPESCRIPT.md).

Not implemented in v1alpha1: `finally`, handlers, or event triggers. Parallelism is expressed by independent graph tasks rather than a separate parallel-group construct.
