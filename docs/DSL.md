# Workflow DSL

The current document version is `agentctl.dev/v1alpha1`, with `kind: Workflow`. The generated, authoritative JSON Schema is [`schemas/workflow.schema.json`](../schemas/workflow.schema.json). YAML documents are limited to 1 MiB and reject unknown fields.

`metadata` contains the name, description, and labels. `spec` contains typed inputs/outputs; providers; bounded agents; actions; tool contracts; ordered tasks; policy; memory; MCP servers; A2A peers; packs; runtime; and output settings. A task `uses` either `action:<name>` or `agent:<name>`, declares `needs`, an optional `when`, local `vars`, typed `with` input, optional `outputSchema`, retry, timeout, and failure behavior.

Templates use only `${{ inputs.path }}`, `${{ vars.path }}`, `${{ memory.path }}`, and `${{ tasks.task-id.output.path }}`. Conditions additionally allow `not` and equality against a JSON literal or string. Exact templates preserve their JSON type; interpolation into text accepts only scalars. Missing and explicit `null` are different. There is no code execution, function call, indexing, arithmetic, or implicit task dependency.

Task output is JSON. Built-in actions own an object contract, agents can declare provider-enforced `structuredOutput`, and a task can override the complete contract with `outputSchema`. The compiler validates schemas; the runtime validates completed and selectively reused values.

Providers, action environments, and protocol headers use `{ env: NAME }` secret references. Secret names are validated and values never become the workflow document.

`policy.workspaceRoot` is the default boundary for relative file paths. Each `writableRoots` entry may be workspace-relative or an explicit absolute mount such as `/artifacts`. Ordinary reads remain workspace-confined. After a successful authorized mutation, the runtime may read that exact output through its writable-root boundary to ingest the bounded regular file into durable CAS; this does not grant tasks general read access to the external root.

The compiler validates missing references, duplicate tasks, cycles, task-aware templates, tool references, provider capabilities, agent limits, and sequential runtime settings before execution. Ready tasks follow declaration order. `maxConcurrency` must be `1` in this version.

`builtin.shell.exec` captures stdout and stderr concurrently. Its optional `stdoutLimitBytes`, `stderrLimitBytes`, and `combinedOutputLimitBytes` fields default to 1 MiB, 1 MiB, and 2 MiB respectively. Each configured value must be between 1 byte and 16 MiB. `timeoutSeconds` must be between 1 and 86,400. Exceeding an output bound terminates and reaps the process and records a structured failed effect; timeout or cancellation remains an uncertain effect because external changes may already have occurred. These fields are validated identically for workflow and pack actions.

The parser translates a limited unversioned `playbook:` document and emits a migration warning. Use `agentctl migrate old.yaml --write new.yaml`. Legacy pack-backed, MCP, A2A, provider-specific, and broad module configurations need manual migration; see [Migrating from TypeScript](MIGRATING_FROM_TYPESCRIPT.md).

Not implemented in v1alpha1: `foreach`, matrix expansion, parallel groups, routers, loops, sub-workflows, `finally`, handlers, event triggers, or compensation execution. They remain excluded until their deterministic state, merge, and recovery semantics are specified.
