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
| `tasks` | required list | Ordered graph nodes. |
| `policy` | safe defaults | Filesystem, process, network, provider, tool, and approval rules. |
| `memory` | empty | Initial working memory and optional SQLite long-term namespace. |
| `mcpServers` | `{}` | Pinned MCP Streamable HTTP peers. |
| `a2aPeers` | `{}` | Pinned A2A Agent Card peers. |
| `packs` | `[]` | Local reviewed pack references. |
| `runtime` | sequential defaults | Runtime controls. `maxConcurrency` must be `1`. |
| `output` | defaults | Output presentation contract. |

## Tasks

Each task requires `id` and `uses`. `uses` is `action:name` or `agent:name`.

| Field | Default | Validation |
| --- | --- | --- |
| `needs` | `[]` | Every ID must exist; cycles fail. |
| `when` | true | Constrained boolean/equality expression. |
| `vars` | `{}` | Task-local JSON values. |
| `with` | `{}` | Typed action or agent input. |
| `retry` | bounded default | Only definitive retry-safe failures may repeat. |
| `timeoutSeconds` | action or agent default | Must be within the implementation bound. |
| failure behavior | fail | Unsupported dynamic control flow is rejected. |

Ready tasks run in YAML declaration order. There is no `foreach`, matrix, loop, router, sub-workflow, handler, or parallel group in this version.

## Agents

An agent requires `provider` and `model`. Defaults are `maxTurns: 8`, `maxToolCalls: 16`, `maxOutputTokens: 2048`, and `timeoutSeconds: 120`. Set tighter values for known work. Optional fields include instructions or `instructionsFile`, variables, tools, retry, reasoning, structured output, usage limits, and provider-specific options.

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

An exact template preserves objects, arrays, booleans, numbers, strings, and null. Text interpolation accepts scalars. Conditions add `not` and equality against a JSON literal or string. There is no code execution, arithmetic, arbitrary function, indexing, or implicit dependency.

## Secret references

Provider credentials, action environment values, and protocol headers use `{ env: NAME }`. The environment name is stored in the workflow, but the value is resolved only at the adapter boundary and must be allowed by policy.

## Example and validation

See `examples/v1/dataflow.yaml` for typed inputs and task outputs. From the repository root:

```text
agentctl check examples/v1/dataflow.yaml
agentctl plan examples/v1/dataflow.yaml
agentctl run examples/v1/dataflow.yaml --db /tmp/dataflow.db --output json --color never
```

Related guides: [Workflow authoring](../guides/WORKFLOW_AUTHORING.md), [Policies](../POLICIES.md), [Tools](../TOOLS.md), and [Workflow DSL](../DSL.md).
