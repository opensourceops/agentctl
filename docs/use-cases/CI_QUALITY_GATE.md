# CI quality gate

## Problem

A generic OCI pipeline step must convert typed evidence into a stable success or failure, emit one machine-readable result, and preserve enough state for diagnosis.

## Why agentctl fits

The workflow separates inputs from a deterministic assertion. The CLI returns a stable exit code and JSON envelope, while the CI platform retains `/state` and `/artifacts`.

## Complete workflow

Source: `examples/docs/ci-quality-gate/workflow.yaml`.

<!-- agentctl-include: examples/docs/ci-quality-gate/workflow.yaml language=yaml -->

## Run it

```text
agentctl run examples/docs/ci-quality-gate/workflow.yaml \
  --db /tmp/ci-quality-gate.db --output json --color never
```

The default exits `0` with verdict `pass`. Run with `--input checksPassed=false` to exercise the failed gate and exit `4`.

## State and security

Use ordinary typed inputs for non-secret gate evidence. Inject provider secrets
only through typed environment or mounted-file references. Archive the database
on failure only when its potentially confidential content is protected.

## Current limitation

This workflow does not run tests itself. A surrounding pipeline can supply results, or a reviewed `builtin.shell.exec` action can run a specifically allowed executable.
