# Run your first deterministic workflow

You will validate, plan, run, and inspect a credential-free workflow from a clean directory. The run performs one deterministic assignment, persists its history to SQLite, and returns a typed output.

## Prerequisites

- A built or installed `agentctl` binary
- No provider credential

## 1. Create a clean workspace

Create an empty directory and save the complete document below as `workflow.yaml`. The same document is checked as `examples/v1/hello.yaml` in source verification.

```sh
mkdir agentctl-first-run
cd agentctl-first-run
```

```yaml
apiVersion: agentctl.dev/v1
kind: Workflow
metadata:
  name: hello
  description: Deterministic hello world with typed inputs and outputs.
spec:
  inputs:
    name: world
  outputs:
    greeting: "${{ tasks.greet.output.output.message }}"
  actions:
    assign:
      kind: builtin.assign
  tasks:
    - id: greet
      uses: action:assign
      with:
        message: "hello, ${{ inputs.name }}"
```

`spec.inputs.name` is editable data. The `greet` task uses a deterministic assignment, and `spec.outputs.greeting` exposes its result. This first workflow grants no host process or network access. Its only persistent runtime output is the local history database.

## 2. Validate the workflow

```text
agentctl check workflow.yaml
```

Expected output:

```text
valid: hello (1 tasks)
```

`check` validates strict YAML, references, templates, policies, and provider capabilities. It does not create a database or execute an effect.

## 3. Inspect the plan

```text
agentctl plan workflow.yaml
```

The plan reports task order `greet`, one effect, and `FullyPredictable`. A plan explains what the compiler knows. It does not claim to predict model or remote-system results.

## 4. Run the workflow

```text
agentctl run workflow.yaml --db .agentctl/runtime.db --output json --color never
```

This command writes durable state to `.agentctl/runtime.db`. It makes no network call and needs no credential. The final envelope has `apiVersion: agentctl.dev/cli/v1`, state `succeeded`, and output:

```json
{"greeting":"hello, world"}
```

Copy the returned `runId` for inspection. To change the input for a fresh run:

```sh
agentctl run workflow.yaml --input name=platform --db .agentctl/runtime.db --output json --color never
```

The new result is `{"greeting":"hello, platform"}`. Each invocation creates a distinct run; the first run remains inspectable.

## 5. Inspect durable history

```text
agentctl inspect RUN_ID --db .agentctl/runtime.db --output json --color never
agentctl db stats --db .agentctl/runtime.db --output json --color never
```

Replace `RUN_ID` with the identifier from the run result. Inspection shows the run, task state, effects, checkpoints, audit records, and trace correlation. The database may contain workflow inputs and outputs, so protect it as sensitive operational data.

## 6. Understand artifacts

This workflow declares no file artifact. A workflow that uses `builtin.write` writes only beneath a policy-approved writable root. The runtime records the write as an effect, while the file itself stays in the workspace or mounted artifact directory.

## Verify the result

The tutorial is complete when all of these are true:

- `check` reports one valid task.
- `plan` reports `FullyPredictable`.
- `run` exits `0` with `hello, world`.
- `.agentctl/runtime.db` exists.
- `inspect` returns the same successful run.

If a command fails, read [Troubleshooting](TROUBLESHOOTING.md).

## Replay and cleanup

```sh
agentctl replay RUN_ID --db .agentctl/runtime.db --output json --color never
```

Replay reconstructs recorded output without invoking an executor. After inspection, remove this tutorial's `agentctl-first-run` directory if you no longer need its history; do not remove an operational database to recover a failed production run.

## Next step

Run [your first bounded agent workflow](FIRST_AGENT_WORKFLOW.md) without a paid API key.
