# ADR 0006: Schedulable runtime and non-interactive contract

Status: accepted

## Decision

`agentctl` is a schedulable runtime, not a scheduler. External platforms own triggers, calendars, leader election, overlap policy, log retention, and job lifecycle. The CLI owns one bounded run, durable SQLite history, effect safety, cancellation, outputs, and recovery.

Non-interactive execution never prompts or auto-approves. The default approval behavior persists the request, pauses the run, emits run/trace correlation, and exits `3`. An operator resolves the approval and invokes `resume`. `deny_approval` and `fail` are stricter explicit modes.

Machine output is one `agentctl.dev/cli/v1` final envelope. Inputs come from JSON, an input file, or repeated `KEY=VALUE` arguments; provider secrets remain environment references. Separate runs can share a SQLite database, but external schedulers must prevent overlapping effects when the target resource requires serialization.

## Consequences

Cron, systemd, Kubernetes, and CI can use normal process semantics without hidden terminal waits. The product avoids an eventing/distributed-control-plane surface. A future schedule-run key may improve deduplication, but it cannot replace external overlap controls or effect idempotency.
