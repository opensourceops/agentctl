# YAML reference

The generated [workflow JSON Schema](../../schemas/workflow.schema.json) is authoritative. This page explains the field groups, defaults, and validation behavior that matter when writing YAML.

## Document envelope

| Field | Required | Meaning |
| --- | --- | --- |
| `apiVersion` | yes | Must be `agentctl.dev/v1alpha1`. |
| `kind` | yes | Must be `Workflow`. |
| `metadata.name` | yes | Stable human-readable workflow name. |
| `metadata.description` | no | Short purpose. |
| `metadata.labels` | no | String metadata map. |
| `spec` | yes | Workflow declarations and ordered tasks. |

Unknown fields fail. Documents, ordinary input files, packs, direct reads, existing write targets, and instruction files are limited to 1 MiB.

## Workflow declarations

| `spec` field | Default | Purpose |
| --- | --- | --- |
| `inputs` | `{}` | Default JSON values supplied to templates. |
| `outputs` | `{}` | Final values selected from inputs, memory, variables, or task outputs. |
| `providers` | `{}` | Named fake, OpenAI, Azure OpenAI, Anthropic, or Google adapters. |
| `agents` | `{}` | Named bounded model executors. |
| `actions` | `{}` | Named deterministic or protocol actions. |
| `tools` | `{}` | Strict model-callable tool contracts. |
| `subworkflows` | `{}` | Semantically versioned reusable task graphs with typed input and output boundaries. |
| `compensation` | manual, policy approval | Best-effort compensation trigger and approval behavior. |
| `tasks` | required list | Ordered graph nodes. |
| `policy` | safe defaults | Filesystem, process, network, provider, tool, and approval rules. |
| `memory` | empty | Initial working memory and optional SQLite long-term namespace. |
| `mcpServers` | `{}` | Pinned MCP Streamable HTTP peers. |
| `a2aPeers` | `{}` | Pinned A2A Agent Card peers. |
| `packs` | `[]` | Local reviewed pack references. |
| `runtime` | bounded defaults | Runtime controls. `maxConcurrency` defaults to `1` and accepts `1` through `64`. |
| `output` | defaults | Output presentation contract. |

## Tasks

Each task requires `id` and `uses`. `uses` is `action:name`, `agent:name`,
`workflow:name`, or `router`.

| Field | Default | Validation |
| --- | --- | --- |
| `needs` | `[]` | Every ID must exist; cycles fail. |
| `foreach` | none | Static typed `items`, binding `as`, and `maxItems`. Mutually exclusive with `matrix`; maximum 256 children. |
| `matrix` | none | Static `axes` Cartesian product and `maxItems`. Axis names are template-safe identifiers; maximum 256 children. |
| `route` | required for `uses: router` | Exact typed `select`, unique typed cases, enumerated destinations, and optional default destinations. Every destination must depend on the router. |
| `loop` | none | Required `maxIterations` from 1 through 64, exact typed `while`, and optional typed `initial` value. Mutually exclusive with `when`, `foreach`, `matrix`, and `route`. |
| `memoryWrites` | inferred or `[]` | Working-memory keys. Literal memory-write keys are inferred; templated keys require an explicit set. Unordered overlaps fail when concurrency is greater than one. |
| `when` | true | Constrained boolean/equality expression. |
| `vars` | `{}` | Task-local JSON values. |
| `with` | `{}` | Typed action or agent input. |
| `outputSchema` | action-owned object or agent structured contract | Valid JSON Schema checked at task completion and selective-repair reuse. |
| `retry` | bounded default | Only definitive retry-safe failures may repeat. |
| `timeoutSeconds` | action or agent default | Must be within the implementation bound. |
| `compensate` | none | Named effectful action, typed `with`, bounded retry, and timeout. Valid only on a potentially mutating task. |
| failure behavior | fail | Unsupported dynamic control flow is rejected. |

Ready tasks are selected in YAML declaration order up to `maxConcurrency`.
They read isolated durable snapshots and commit in compiled order. There is no
runtime or model-controlled expansion. Static `foreach` and `matrix` tasks
compile to inspectable child tasks and a parent aggregate. Bounded loops
compile to a sequential child chain and parent aggregate. Sub-workflows compile
to namespaced ordinary tasks with typed input and output boundaries. There is
no handler or separate parallel group in this version. Compensation is planned
after a terminal run and executes declared inverse actions in reverse graph
order through an ordinary source-linked durable run.

## Agents

An agent requires `provider` and `model`. Defaults are `maxTurns: 8`, `maxToolCalls: 16`, `maxOutputTokens: 2048`, and `timeoutSeconds: 120`. Set tighter values for known work. Optional fields include instructions or `instructionsFile`, variables, tools, retry, reasoning, structured output, usage limits, and provider-specific options.

`structuredOutput` asks the provider for typed JSON and becomes the default task output contract. A task-level `outputSchema` can define the complete task contract explicitly. An agent result that feeds downstream tasks must have one of these contracts before it can be reused by selective repair. Schema documents are compiled when the workflow is checked; values are validated both when completed and when reused.

Capability negotiation happens during compilation. A provider must explicitly support every requested feature.

## Actions

Supported action kinds:

- `builtin.assign`
- `builtin.assert`
- `builtin.read`
- `builtin.write`
- `builtin.shell.exec`
- `builtin.memory.read`
- `builtin.memory.write`
- `builtin.long_term_memory.read`
- `builtin.long_term_memory.write`
- `mcp.call`
- `a2a.delegate`

`builtin.shell.exec` uses a direct executable and argument list. Output defaults are 1 MiB per stream and 2 MiB combined, with a maximum configured value of 16 MiB. Its maximum timeout is 86,400 seconds.

## Tools

A tool requires `kind`, description, strict input and output JSON Schema, capability, risk, effect class, idempotency, retry safety, timeout, and approval behavior. Built-in tool executors are workspace read, workspace write, and echo. Declared semantics must match the built-in kind.

## Templates and conditions

Allowed template roots are:

```text
${{ inputs.path }}
${{ vars.path }}
${{ memory.path }}
${{ tasks.task-id.output.path }}
```

An exact template preserves objects, arrays, booleans, numbers, strings, and null. Text interpolation accepts scalars. Conditions add `not`, type-sensitive `==` and `!=`, and numeric `<`, `<=`, `>`, and `>=`. There is no code execution, arithmetic, arbitrary function, indexing, or implicit dependency.

## Secret references

Provider credentials, action environment values, and protocol headers use
`{ env: NAME }`, `{ file: PATH }`, or a bounded `{ process: ... }` reference.
The source description is stored in the workflow, but the value is resolved
only at the execution boundary. File and process sources require explicit
`secretFileRoots` or `secretProcessAllowlist` policy. See
[Secret references](../guides/SECRET_REFERENCES.md).

## Example and validation

See `examples/v1/dataflow.yaml` for typed inputs and task outputs. From the repository root:

```text
agentctl check examples/v1/dataflow.yaml
agentctl plan examples/v1/dataflow.yaml
agentctl run examples/v1/dataflow.yaml --db /tmp/dataflow.db --output json --color never
```

Related guides: [Workflow authoring](../guides/WORKFLOW_AUTHORING.md), [Matrix
and foreach](../guides/MATRIX_AND_FOREACH.md), [Conditions and
routers](../guides/CONDITIONS_AND_ROUTERS.md), [Bounded
loops](../guides/BOUNDED_LOOPS.md), [Reusable
sub-workflows](../guides/SUB_WORKFLOWS.md),
[Compensation](../guides/COMPENSATION.md), [Secret
references](../guides/SECRET_REFERENCES.md), [Policies](../policies.md),
[Tools](../TOOLS.md), and [Workflow DSL](../DSL.md).
