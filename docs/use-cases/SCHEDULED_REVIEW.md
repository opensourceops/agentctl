# Scheduled operational review

## Problem

An external scheduler needs to run a bounded check, retain state, and collect a report without an interactive session.

## Why agentctl fits

The CLI has a non-interactive process contract, explicit state and artifact paths, durable failures, structured output, and safe cancellation. Cron, systemd, or Kubernetes owns the schedule and overlap rule.

## Complete workflow

Source: `examples/docs/scheduled-review/workflow.yaml`.

<!-- agentctl-include: examples/docs/scheduled-review/workflow.yaml language=yaml -->

## Run it

From a copy of the example directory:

```text
mkdir -p artifacts .agentctl
agentctl run workflow.yaml --db .agentctl/runtime.db \
  --timeout-seconds 300 --output json --color never
```

Expected output declares `artifacts/operational-review.txt`. The file contains `scheduled operational review passed`.

## State and security

Persist the database and artifact directory with restrictive permissions. Configure `flock`, systemd serialization, or Kubernetes `concurrencyPolicy: Forbid` when overlapping external effects are unsafe.

## Current limitation

`agentctl` is a schedulable runtime, not a scheduling service. It does not provide clocks, calendars, distributed leases, or log rotation.
