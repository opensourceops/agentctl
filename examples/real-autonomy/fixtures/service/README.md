# Ledger Sync Service

Ledger Sync Service receives partner webhook events and writes normalized records to Postgres.

## Operations

- Deployments are manual and happen from a maintainer laptop.
- The service publishes `/healthz` and `/metrics`.
- Alerts are sent to a shared Slack channel.

## Known Gaps

- There is no documented on-call escalation policy.
- Backups exist at the database layer, but there is no backup restore drill.
- The runbook documents startup and rollback, but not recovery validation.
