# Ops Readiness Report

## Summary
The service repository has basic documentation and environment examples, but key operational practices are not documented or scheduled. Specifically, an on-call escalation policy is missing and there is no backup restore drill or restore validation scheduled.

## Evidence
- README.md
  - Path: ./fixtures/service/README.md
  - Contains service overview and links to operational docs but does not document escalation or restore drills.
- docs/runbook.md
  - Path: ./fixtures/service/docs/runbook.md
  - Contains the following explicit findings:
    - "There is no documented on-call escalation policy."
    - "Backups exist at the database layer, but there is no backup restore drill."
    - "No restore validation drill is scheduled."

## Risks
- Without a documented on-call escalation policy, incidents may suffer from delayed or inconsistent escalation, increasing time-to-resolution and risk of human error.
- Without a backup restore drill and no scheduled restore validation, backups may be unrecoverable or unusable in a real outage, risking prolonged data loss or service downtime.
- Lack of scheduled drills and formal escalation increases organizational risk during incidents, complicates post-incident reviews, and may impact compliance requirements.

## Recommended Next Steps
1. Define and publish an on-call escalation policy in docs/runbook.md (or a linked operational policy) that includes roles, contact methods, escalation timelines, and fallback contacts.
2. Implement and document a backup restore drill:
   - Create a runbook with step-by-step restore procedures.
   - Schedule regular restore drills (at least quarterly) and track outcomes.
3. Add automated restore validation checks (and schedule them) to ensure restorations are tested and verified after drills.
4. Update README.md to link to the runbook and explicitly reference the on-call policy and backup/restore drill schedule.