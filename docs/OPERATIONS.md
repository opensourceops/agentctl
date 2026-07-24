# Scheduled and unattended operation

Scheduling belongs to the external platform. `agentctl` owns deterministic execution, SQLite history, overlap-safe database access, effects, outputs, recovery, and diagnostics; it does not own clocks, calendars, leader election, log rotation, or distributed leases.

## Non-interactive contract

- Do not pass `--interactive` from cron or CI.
- Use `--output json --color never` for one parseable final document on stdout. Errors use the same versioned envelope on stderr.
- Set explicit `--workspace`, `--db`, artifact inputs, and `--timeout-seconds`.
- A pending approval is persisted and exits `3`; it never waits on stdin. Use `approvals list`, an operator-controlled `approve` or `reject`, and then `resume` with the same database and workspace.
- Success is `0`, validation is `2`, policy/approval is `3`, run failure is `4`, persistence is `5`, provider/protocol failure is `6`, and cancellation is `130`.
- Output/error correlation includes a run ID and trace ID whenever a run exists.

## Cron

Use absolute paths and an external overlap lock when two schedules must not affect the same resource:

```text
*/15 * * * * /usr/bin/flock -n /var/lib/agentctl/report.lock /usr/local/bin/agentctl run /etc/agentctl/report.yaml --workspace /srv/app --db /var/lib/agentctl/runtime.db --inputs-file /etc/agentctl/inputs.json --timeout-seconds 600 --output json --color never >>/var/log/agentctl/report.jsonl 2>>/var/log/agentctl/report.err
```

The administrator owns log rotation and restrictive file permissions. Provider keys belong in the scheduler's protected environment, never in the crontab command line.

## systemd timer

```ini
# /etc/systemd/system/agentctl-report.service
[Unit]
Description=Run the reviewed agentctl report workflow

[Service]
Type=oneshot
User=agentctl
EnvironmentFile=/etc/agentctl/provider.env
ExecStart=/usr/local/bin/agentctl run /etc/agentctl/report.yaml --workspace /srv/app --db /var/lib/agentctl/runtime.db --inputs-file /etc/agentctl/inputs.json --timeout-seconds 600 --output json --color never
ReadWritePaths=/var/lib/agentctl /srv/agentctl-artifacts
NoNewPrivileges=true
PrivateTmp=true
```

```ini
# /etc/systemd/system/agentctl-report.timer
[Unit]
Description=Schedule the agentctl report workflow

[Timer]
OnCalendar=*:0/15
Persistent=true
Unit=agentctl-report.service

[Install]
WantedBy=timers.target
```

A oneshot service has one active invocation at a time. Use distinct databases only when independent histories are intended.

## Recovery

1. Capture the final JSON/error envelope and run/trace IDs.
2. Run `agentctl inspect RUN_ID --db PATH --output json`.
3. Resolve a pending approval, then `resume`; never use `fork` as an implicit retry.
4. Use `replay` for a no-effect reconstruction of a terminal run.
5. Use `repair TARGET SOURCE --from TASK --plan` before executing a corrected terminal workflow from a task boundary.
6. Use `fork` for a broader new run that may execute fresh effects.
7. For an uncertain effect, reconcile the remote system first. The runtime intentionally refuses unsafe resume or repair.
8. Verify retained bytes with `agentctl artifacts --db PATH verify --all`; export a digest with `agentctl artifacts --db PATH export DIGEST DESTINATION`.

Repair planning exits `3` when compatibility or effect safety blocks reuse. Read `blockedReuse`, choose an earlier/additional root, restore a verified artifact, or reconcile an effect. Do not bypass the plan with a fresh fork unless repeating all effects is an intentional operator decision.

Use `agentctl gc --db PATH --older-than-days N` for expired memory and old terminal histories. Then use `agentctl artifacts --db PATH gc --older-than-days N --dry-run` to preview unreferenced blobs before running it without `--dry-run`. SQLite WAL files and the sibling artifact root belong together during backup. A future schedule-run key may improve deduplication; today the external scheduler owns overlap prevention.
