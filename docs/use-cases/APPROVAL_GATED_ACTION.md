# Approval-gated action

## Problem

A workflow may propose a mutation, but an operator must review the exact requested effect before it runs.

## Why agentctl fits

Policy sits outside the model and creates a durable approval record. Non-interactive execution pauses and exits `3`; it never prompts, auto-approves, or loses the pending request.

## Complete workflow

Source: `examples/v1/approval.yaml`.

<!-- agentctl-include: examples/v1/approval.yaml language=yaml -->

## Run it

```text
agentctl run examples/v1/approval.yaml --db /tmp/approval.db \
  --output json --color never
agentctl approvals list RUN_ID --db /tmp/approval.db
agentctl approvals approve APPROVAL_ID --db /tmp/approval.db \
  --actor operator@example.invalid --reason "Reviewed file write"
agentctl resume RUN_ID --db /tmp/approval.db --output json --color never
```

The first command exits `3` and does not write the file. The resumed run writes it only after approval.

## State and security

The approval includes redacted input, tool or action, capability, risk, expected effect, actor, reason, run, task, and trace correlation. Approval is a decision point, not proof that content is safe.

## Current limitation

The CLI stores operator-provided identity text but does not provide a hosted identity or role system. The invoking platform must authenticate and authorize the operator.
