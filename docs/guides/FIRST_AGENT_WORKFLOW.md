# Run your first bounded agent workflow

You will run a tool-using agent with the deterministic fake provider. The model path is scripted, but the workflow exercises the real compiler, agent loop, tool policy, tool schemas, effect ledger, SQLite store, assertion, and artifact writer.

## Prerequisites

- An installed `agentctl` binary
- No provider credential

## 1. Create the complete example

Create `agentctl-first-agent` with this layout:

```text
agentctl-first-agent/
  workflow.yaml
  fixture/service.txt
  artifacts/
```

Save this as `fixture/service.txt`:

```text
service=agentctl-acceptance
status=ready
marker=READ_TOOL_CONFIRMED
```

Save the following complete document as `workflow.yaml`, then run the remaining commands inside `agentctl-first-agent`:

```yaml
apiVersion: agentctl.dev/v1
kind: Workflow
metadata:
  name: acceptance-mock-tool
spec:
  inputs:
    reportPath: artifacts/mock-report.txt
  outputs:
    verdict: "${{ tasks.inspect.output.text }}"
    artifact: "${{ inputs.reportPath }}"
  providers:
    fake:
      kind: fake
  policy:
    workspaceRoot: .
    writableRoots: [artifacts]
    approval: never
  tools:
    read_fixture:
      kind: builtin.workspace.read
      description: Read a UTF-8 file inside the authorized workspace.
      inputSchema:
        type: object
        properties:
          path: { type: string }
        required: [path]
        additionalProperties: false
      outputSchema:
        type: object
        properties:
          path: { type: string }
          content: { type: string }
          bytes: { type: integer }
          sha256: { type: string }
        required: [path, content, bytes, sha256]
        additionalProperties: false
      capability: filesystem.read
      risk: low
      effectClass: observe
      idempotency: idempotent
      retrySafe: true
      timeoutSeconds: 5
      approval: never
  agents:
    inspector:
      provider: fake
      model: scripted
      instructions: Read the fixture and report its marker.
      tools: [read_fixture]
      maxTurns: 2
      maxToolCalls: 1
      maxOutputTokens: 32
      timeoutSeconds: 5
      providerOptions:
        toolInput: { path: fixture/service.txt }
        finalText: AGENTCTL_MOCK_FIXTURE_VERIFIED
  actions:
    assert:
      kind: builtin.assert
    write:
      kind: builtin.write
  tasks:
    - id: inspect
      uses: agent:inspector
      with:
        prompt: Use read_fixture before answering.
    - id: verify
      uses: action:assert
      needs: [inspect]
      with:
        that: "${{ tasks.inspect.output.text == 'AGENTCTL_MOCK_FIXTURE_VERIFIED' }}"
        message: the mock provider did not complete its tool continuation
    - id: report
      uses: action:write
      needs: [inspect, verify]
      with:
        path: "${{ inputs.reportPath }}"
        content: "${{ tasks.inspect.output.text }}"
```

The repository acceptance suite executes these same files outside the source tree. No shared helper or provider credential is required.

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

## Replay and cleanup

```sh
agentctl replay RUN_ID --db .agentctl/runtime.db --output json --color never
```

Replay reconstructs the recorded result with no new provider request or tool call. After inspecting the report, remove only this disposable directory when its history is no longer needed.

## Move to a real provider deliberately

The fake provider demonstrates orchestration and contracts; its scripted marker is not evidence that a model understood a file. The [CI diagnosis cookbook](../../examples/devops/01-ci-diagnosis/README.md) offers a separately labeled OpenAI workflow with bounded structured decisions. Keep credentials in the existing runtime secret mechanism, and review network policy, request and token ceilings, pricing assumptions, and the persisted data boundary before a paid run.

## Troubleshooting

- `binary not found`: use the absolute path to `target/debug/agentctl` or install the CLI.
- `path is outside workspace`: run from the copied example directory.
- artifact permission error: make `artifacts/` writable by the current user.
- live authentication failure: run `agentctl auth check` against the workflow without printing the secret.

## Next step

Read [Workflow authoring](WORKFLOW_AUTHORING.md) to replace the scripted journey with your own reviewed workflow.
