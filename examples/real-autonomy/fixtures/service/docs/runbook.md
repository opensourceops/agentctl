# Service Runbook

## Startup

1. Export environment variables from `.env.production`.
2. Run the release job.
3. Verify `/healthz` responds with `200`.

## Rollback

1. Re-run the previous release.
2. Confirm error rate returns to baseline.

## Notes

- Primary alerts route to Slack `#ledger-sync-alerts`.
- Database snapshots run nightly.
- No restore validation drill is scheduled.
