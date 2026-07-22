# Author a workflow

This guide teaches the YAML model in the order you need it. The generated JSON Schema is the field authority, but start with the execution model rather than the schema dump.

## 1. Give the workflow an identity

Every document has a strict versioned envelope:

```yaml
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata:
  name: repository-check
  description: Check a repository with reviewed automation.
spec:
  tasks: []
```

Unknown fields fail validation. The document limit is 1 MiB.

## 2. Declare inputs

`spec.inputs` defines default JSON values. A caller can override them with `--inputs`, `--inputs-file`, or repeated `--input KEY=VALUE`.

```yaml
spec:
  inputs:
    reportPath: artifacts/report.txt
```

Templates can read `${{ inputs.reportPath }}`. An exact template preserves its JSON type. Interpolation into text accepts scalars only.

## 3. Define tasks

Tasks are the graph nodes. Their list order is also the deterministic tie-break order for tasks whose dependencies are ready.

```yaml
tasks:
  - id: inspect
    uses: action:read
    with:
      path: README.md
```

## 4. Add dependencies

Use `needs` for graph edges and task-output availability:

```yaml
- id: verify
  uses: action:assert
  needs: [inspect]
  with:
    that: "${{ tasks.inspect.output.content }}"
    message: README was empty
```

The compiler rejects missing references, implicit dependencies, duplicate IDs, and cycles.

## 5. Choose deterministic actions

Built-in actions cover assignment, assertion, file read and write, direct process execution, working memory, SQLite long-term memory, MCP calls, and A2A delegation. Declare each action once, then reference it with `action:name`.

Process execution uses a command plus direct argument vector. It never inserts an implicit shell and receives only explicitly allowed environment variables.

## 6. Add a bounded agent

An agent names a provider, model, instructions, tools, and hard bounds:

```yaml
providers:
  fake:
    kind: fake
agents:
  reviewer:
    provider: fake
    model: scripted
    instructions: Review only the supplied evidence.
    tools: [read_repository]
    maxTurns: 2
    maxToolCalls: 1
    maxOutputTokens: 128
    timeoutSeconds: 10
```

The model does not own the graph, policy, or persistence.

## 7. Define tool contracts

A model sees only tools listed on its agent. Each tool requires strict input and output schema, capability, risk, effect class, idempotency, retry safety, timeout, and approval requirement. Runtime policy makes the final authorization decision.

## 8. Declare outputs

Workflow outputs select completed task data:

```yaml
outputs:
  report: "${{ tasks.report.output.path }}"
```

Machine output wraps declared values in the versioned CLI envelope.

## 9. Constrain policy

Policy owns workspace roots, writable roots, environment names, network hosts, processes, providers, tools, and approvals. Allowlists are application controls, not an operating-system sandbox.

Start with the minimum grant. Add a host, writable root, executable, or secret name only when a verified task needs it.

## 10. Plan for state and recovery

Choose an explicit database path for scheduled or CI runs. A confirmed effect can be reused during resume. An effect that started without a confirmed result becomes uncertain and stops automatic recovery. Recorded replay calls no executor. Fork intentionally permits fresh effects.

## Validate your workflow

From the directory containing the workflow:

```text
agentctl check workflow.yaml
agentctl plan workflow.yaml
agentctl run workflow.yaml --check --diff --db .agentctl/preview.db
```

The preview may write run history to its database, but it does not perform filesystem, process, remote, or model mutation. Read [Workflow DSL](../DSL.md) and the [YAML reference](../reference/YAML.md) for the complete contract.
