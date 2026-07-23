# Run your first deterministic workflow

You will validate, plan, run, and inspect a credential-free workflow from a clean directory. The run performs one deterministic assignment, persists its history to SQLite, and returns a typed output.

## Prerequisites

- A built or installed `agentctl` binary
- The `agentctl` source checkout
- No provider credential

## 1. Create a clean workspace

From the repository root:

```text
mkdir -p /tmp/agentctl-first-run
cp examples/v1/hello.yaml /tmp/agentctl-first-run/workflow.yaml
cd /tmp/agentctl-first-run
```

The copy is the canonical checked example. The commands write only beneath the temporary workspace.

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

Copy the returned `runId` for inspection.

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

## Next step

Run [your first bounded agent workflow](FIRST_AGENT_WORKFLOW.md) without a paid API key.
