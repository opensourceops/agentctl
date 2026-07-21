# Ops Readiness Report

## Summary
The Ledger Sync Service exposes health and metrics, has alerts to Slack, and nightly database snapshots. Operational gaps include the absence of a documented on-call escalation policy and no validated backup restore process.

## Evidence
- README.md
  - Contains: "There is no documented on-call escalation policy."
  - Contains: "Backups exist at the database layer, but there is no backup restore drill."
  - Path: ./fixtures/service/README.md
- docs/runbook.md
  - Contains: "No restore validation drill is scheduled."
  - Path: ./fixtures/service/docs/runbook.md

Concrete file excerpts:
- README.md: "There is no documented on-call escalation policy."
- README.md: "Backups exist at the database layer, but there is no backup restore drill."
- docs/runbook.md: "No restore validation drill is scheduled."

## Risks
- Without a documented on-call escalation policy, incidents may suffer delayed triage, unclear ownership, and inconsistent escalation paths.
- Backups without a restore drill and no restore validation risk prolonged or failed recovery after data loss, undermining RTO/RPO guarantees.
- Manual deployments from a maintainer laptop increase human error and single-person operational risk.

## Recommended Next Steps
1. Create and publish a documented on-call escalation policy (roles, escalation path, SLAs) and link it from README and runbook.
2. Design, schedule, and run a backup restore drill (full restore from nightly snapshot) and document the playbook.
3. Add automated restore validation to the runbook and schedule regular drills (at least quarterly).
4. Automate deployments (CI/CD) to reduce single-operator risk and document rollback/runbook steps for automated flows.