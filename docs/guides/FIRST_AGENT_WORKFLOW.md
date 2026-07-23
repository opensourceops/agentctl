# Run your first bounded agent workflow

You will run a tool-using agent with the deterministic fake provider. The model path is scripted, but the workflow exercises the real compiler, agent loop, tool policy, tool schemas, effect ledger, SQLite store, assertion, and artifact writer.

## Prerequisites

- A built `agentctl` binary at `target/debug/agentctl`, or an installed binary
- The source checkout
- No provider credential

## 1. Copy the verified journey

From the repository root:

```text
cp -R examples/acceptance/mock-tool /tmp/agentctl-first-agent
cd /tmp/agentctl-first-agent
```

The directory contains `workflow.yaml`, `fixture/service.txt`, and an empty artifact directory. The repository acceptance suite copies and runs the same journey outside the source tree.

## 2. Read the boundary

The workflow gives the `inspector` agent one tool, `read_fixture`. Its input and output use strict JSON Schema. Policy limits file access to the workspace, grants no mutation to the agent, and needs no network host or environment secret.

The fake provider is configured to request `fixture/service.txt`, then return the fixed text `AGENTCTL_MOCK_FIXTURE_VERIFIED`. This makes the learning path deterministic while preserving the real tool continuation protocol.

## 3. Validate and plan

```text
agentctl check workflow.yaml
agentctl plan workflow.yaml
```

These commands make no network call and write no runtime state. The plan can identify the graph and declared effects, but the agent task still requires execution.

## 4. Run the agent

```text
agentctl run workflow.yaml --db .agentctl/runtime.db --output json --color never
```

Expected final output contains:

```json
{
  "artifact": "artifacts/mock-report.txt",
  "verdict": "AGENTCTL_MOCK_FIXTURE_VERIFIED"
}
```

The run writes `.agentctl/runtime.db` and `artifacts/mock-report.txt`. The `verify` task deterministically rejects any unexpected verdict.

## 5. Inspect the agent and tool records

```text
agentctl inspect RUN_ID --db .agentctl/runtime.db --output json --color never
```

Look for the provider session, tool call, effect correlation, task transitions, assertion result, artifact write, and run trace ID. The durable record lets you distinguish model output from deterministic verification.

## Optional: use OpenAI

Only after the credential-free path works, review `examples/v1/openai-live.yaml`. Export the credential through the environment, never through a flag or YAML value:

```text
export OPENAI_API_KEY="your-provider-secret"
agentctl check examples/v1/openai-live.yaml
agentctl run examples/v1/openai-live.yaml --db .agentctl/openai.db --output json --color never
```

The live command makes a paid network request to `api.openai.com` and writes provider results to the database. Do not run it in normal documentation verification. Remove the variable from the shell when finished.

## Troubleshooting

- `binary not found`: use the absolute path to `target/debug/agentctl` or install the CLI.
- `path is outside workspace`: run from the copied example directory.
- artifact permission error: make `artifacts/` writable by the current user.
- live authentication failure: run `agentctl auth check` against the workflow without printing the secret.

## Next step

Read [Workflow authoring](WORKFLOW_AUTHORING.md) to replace the scripted journey with your own reviewed workflow.
