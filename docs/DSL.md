# Workflow DSL

The current document version is `agentctl.dev/v1alpha1`, with `kind: Workflow`. The generated, authoritative JSON Schema is [`schemas/workflow.schema.json`](../schemas/workflow.schema.json). YAML documents are limited to 1 MiB and reject unknown fields.

`metadata` contains the name, description, and labels. `spec` contains typed inputs/outputs; providers; bounded agents; actions; tool contracts; ordered tasks; policy; memory; MCP servers; A2A peers; packs; runtime; and output settings. A task `uses` either `action:<name>` or `agent:<name>`, declares `needs`, optional bounded `foreach` or `matrix` expansion, optional working-memory `memoryWrites`, an optional `when`, local `vars`, typed `with` input, optional `outputSchema`, retry, timeout, and failure behavior.

Templates use only `${{ inputs.path }}`, `${{ vars.path }}`, `${{ memory.path }}`, and `${{ tasks.task-id.output.path }}`. Conditions additionally allow `not` and equality against a JSON literal or string. Exact templates preserve their JSON type; interpolation into text accepts only scalars. Missing and explicit `null` are different. There is no code execution, function call, indexing, arithmetic, or implicit task dependency.

Task output is JSON. Built-in actions own an object contract, agents can declare provider-enforced `structuredOutput`, and a task can override the complete contract with `outputSchema`. The compiler validates schemas; the runtime validates completed and selectively reused values.

Providers, action environments, and protocol headers use secret references:
`{ env: NAME }`, `{ file: PATH }`, or a bounded `{ process: ... }` reference.
File references require `policy.secretFileRoots`; process references require
`policy.secretProcessAllowlist`. Existing environment references remain
compatible. Resolved values never become the workflow document, effect value,
trace, or inspection output. See [Secret references](guides/SECRET_REFERENCES.md).

`policy.workspaceRoot` is the default boundary for relative file paths. Each `writableRoots` entry may be workspace-relative or an explicit absolute mount such as `/artifacts`. Ordinary reads remain workspace-confined. After a successful authorized mutation, the runtime may read that exact output through its writable-root boundary to ingest the bounded regular file into durable CAS; this does not grant tasks general read access to the external root.

The compiler validates missing references, duplicate tasks, cycles, task-aware templates, tool references, provider capabilities, agent limits, concurrency bounds, and working-memory conflicts before execution. `maxConcurrency` accepts `1` through `64` and defaults to `1`. Independent ready tasks are selected in compiled order, execute against durable isolated memory snapshots, and commit atomically in compiled order. Literal working-memory keys are inferred; templated keys require `memoryWrites`. See [Deterministic parallel tasks](guides/PARALLEL_TASKS.md).

Static `foreach` lists and matrix axes expand at compile time into stable child
tasks plus a parent aggregate. `maxItems` defaults to 32, expansion cannot
exceed 256 children, and model output cannot drive it. Retry and repair can
select the visible child IDs. See [Matrix and foreach tasks](guides/MATRIX_AND_FOREACH.md).

`builtin.shell.exec` captures stdout and stderr concurrently. Its optional `stdoutLimitBytes`, `stderrLimitBytes`, and `combinedOutputLimitBytes` fields default to 1 MiB, 1 MiB, and 2 MiB respectively. Each configured value must be between 1 byte and 16 MiB. `timeoutSeconds` must be between 1 and 86,400. Exceeding an output bound terminates and reaps the process and records a structured failed effect; timeout or cancellation remains an uncertain effect because external changes may already have occurred. These fields are validated identically for workflow and pack actions.

The parser translates a limited unversioned `playbook:` document and emits a migration warning. Use `agentctl migrate old.yaml --write new.yaml`. Legacy pack-backed, MCP, A2A, provider-specific, and broad module configurations need manual migration; see [Migrating from TypeScript](MIGRATING_FROM_TYPESCRIPT.md).

Not implemented in v1alpha1: routers, loops, sub-workflows, `finally`, handlers, event triggers, or compensation execution. Parallelism is expressed by independent graph tasks rather than a separate parallel-group construct.
