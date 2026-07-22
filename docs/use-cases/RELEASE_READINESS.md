# Release-readiness report

## Problem

A release process has deterministic evidence such as tests and a security scan, plus a bounded model summary. Model analysis must not override failed gates.

## Why agentctl fits

Dependencies keep the analysis task behind two assertions. The model can summarize only after deterministic gates pass. A pipeline consumes the final process status and declared output.

## Complete workflow

Source: `examples/docs/release-readiness/workflow.yaml`.

<!-- agentctl-include: examples/docs/release-readiness/workflow.yaml language=yaml -->

## Run it

```text
agentctl run examples/docs/release-readiness/workflow.yaml \
  --db /tmp/release-readiness.db --output json --color never
```

The credential-free example returns decision `RELEASE_EVIDENCE_REVIEWED`. Override `testsPassed=false` to verify that exit `4` prevents the model task from running.

## State and security

The database records which gate failed and whether the analysis task started. A real workflow should pass evidence through reviewed files or typed inputs, not give the model CI credentials or authority to change release state.

## Current limitation

The example uses the fake provider. It demonstrates graph and policy behavior, not a live model quality claim or a release approval system.
