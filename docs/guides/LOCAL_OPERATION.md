# Operate agentctl locally

Use explicit paths and retain the database whenever you may need inspection, approval, resume, replay, repair, or audit evidence.

## Default and custom paths

The CLI defaults to `.agentctl/runtime.db` relative to the current working directory. The workflow workspace defaults to the current directory. Artifact locations come from workflow inputs and policy-approved paths. Override the state path and workspace explicitly for unattended use:

```text
agentctl run config/workflow.yaml \
  --workspace /srv/project \
  --db /var/lib/agentctl/runtime.db \
  --output json --color never
```

This writes the SQLite database and any declared artifacts. Network calls occur only when the workflow uses an allowed provider or protocol.

## Output and exit status

Human output is for terminals. `--output json --color never` emits one versioned final document on stdout; an error uses the same envelope shape on stderr. Exit codes distinguish success, validation, policy or approval, run failure, persistence, remote failure, and cancellation.

`--output jsonl` emits durable provider stream events followed by the final
outcome. Human stream progress uses stderr. Use `--output json` when an
automation requires exactly one final document.

## Interrupt safely

SIGINT and SIGTERM request cancellation, cancel in-flight async work, persist cancellation state, and exit `130`. If dispatch occurred but confirmation did not, the related effect is uncertain. Do not delete the database or immediately rerun an external mutation.

## Inspect a run

```text
agentctl inspect RUN_ID --db /var/lib/agentctl/runtime.db --output json --color never
agentctl approvals list RUN_ID --db /var/lib/agentctl/runtime.db --output json --color never
```

Use the run ID and trace ID when correlating logs. Treat database output as sensitive because prompts, file content, tool output, and remote artifacts may be present even when secret values were redacted.

## Resume, replay, retry, repair, and fork

- Resume continues the same non-terminal run and reuses confirmed effects.
- Retry is bounded within a task and never guesses about an ambiguous effect.
- Recorded replay creates a new record from terminal stored results and calls no executor.
- Repair creates a new source-linked run, reuses only compatible successful tasks before selected roots, and executes every root and descendant from a supplied target workflow.
- Fork creates a new child run and permits fresh effects.

Do not use these terms interchangeably. Read [Durable execution](../DURABLE_EXECUTION.md) before recovering a workflow that may have changed an external system. For a corrected terminal workflow, follow [Repair a failed workflow](repair-a-failed-workflow.md).

## Resolve an approval

Non-interactive execution persists a pending approval and exits `3` by default:

```text
agentctl approvals list RUN_ID --db .agentctl/runtime.db
agentctl approvals approve APPROVAL_ID --db .agentctl/runtime.db \
  --actor operator@example.invalid --reason "Reviewed requested write"
agentctl resume RUN_ID --db .agentctl/runtime.db --output json --color never
```

Reject with `approvals reject` when the proposed effect is not acceptable.

## Back up and retain state

SQLite uses WAL mode. Copy the database and its WAL files as one consistent backup using an SQLite-aware method or during a controlled stop. Keep backups and artifacts according to data classification, not merely run age.

After the retention period:

```text
agentctl gc --db .agentctl/runtime.db --older-than-days 30 --output json --color never
```

Garbage collection deletes eligible terminal history and expired long-term memory. Back up before deletion when the history is audit evidence.
