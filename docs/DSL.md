# Workflow DSL

The current document version is `agentctl.dev/v1alpha1`, with `kind: Workflow`. The generated, authoritative JSON Schema is [`schemas/workflow.schema.json`](../schemas/workflow.schema.json). YAML documents are limited to 1 MiB and reject unknown fields.

`metadata` contains the name, description, and labels. `spec` contains typed inputs/outputs; providers; bounded agents; actions; tool contracts; ordered tasks; policy; memory; MCP servers; A2A peers; packs; runtime; and output settings. A task `uses` either `action:<name>` or `agent:<name>`, declares `needs`, an optional `when`, local `vars`, typed `with` input, retry, timeout, and failure behavior.

Templates use only `${{ inputs.path }}`, `${{ vars.path }}`, `${{ memory.path }}`, and `${{ tasks.task-id.output.path }}`. Conditions additionally allow `not` and equality against a JSON literal or string. Exact templates preserve their JSON type; interpolation into text accepts only scalars. Missing and explicit `null` are different. There is no code execution, function call, indexing, arithmetic, or implicit task dependency.

Providers, action environments, and protocol headers use `{ env: NAME }` secret references. Secret names are validated and values never become the workflow document.

The compiler validates missing references, duplicate tasks, cycles, task-aware templates, tool references, provider capabilities, agent limits, and sequential runtime settings before execution. Ready tasks follow declaration order. `maxConcurrency` must be `1` in this version.

The parser translates a limited unversioned `playbook:` document and emits a migration warning. Use `agentctl migrate old.yaml --write new.yaml`. Legacy pack-backed, MCP, A2A, provider-specific, and broad module configurations need manual migration; see [Migrating from TypeScript](MIGRATING_FROM_TYPESCRIPT.md).

Not implemented in v1alpha1: `foreach`, matrix expansion, parallel groups, routers, loops, sub-workflows, `finally`, handlers, event triggers, or compensation execution. They remain excluded until their deterministic state, merge, and recovery semantics are specified.
