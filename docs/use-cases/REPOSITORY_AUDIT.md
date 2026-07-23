# Repository audit

## Problem

A model needs to inspect a repository and produce a report, but it should see only reviewed files and its conclusion must pass deterministic checks before becoming an artifact.

## Why agentctl fits

The workflow graph and policy are outside the model. A strict read tool limits the agent to the workspace, the fake provider makes the documented journey repeatable, an assertion checks the final marker, and a separate deterministic action writes the report.

## Complete workflow

Source: `examples/acceptance/mock-tool/workflow.yaml`.

<!-- agentctl-include: examples/acceptance/mock-tool/workflow.yaml language=yaml -->

The repository acceptance suite copies this entire directory to a clean workspace and runs it.

## Run it

From the repository root:

```text
cp -R examples/acceptance/mock-tool /tmp/agentctl-repository-audit
cd /tmp/agentctl-repository-audit
agentctl check workflow.yaml
agentctl plan workflow.yaml
agentctl run workflow.yaml --db .agentctl/runtime.db --output json --color never
```

The run needs no credential and makes no network call. Expected output includes verdict `AGENTCTL_MOCK_FIXTURE_VERIFIED` and artifact `artifacts/mock-report.txt`.

## State and security

The database records the provider session, strict tool call, read effect, assertion, write effect, audit events, and trace correlation. The read tool cannot mutate the workspace. Replace the fake provider only after reviewing new credential, network, model, and output risks.

## Current limitation

The checked journey proves orchestration and tool boundaries, not the quality of a live model's repository analysis. Production workflows need task-specific verification stronger than a fixed marker.
