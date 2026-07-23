# Runtime database and migrations

The local SQLite database is both history and part of the correctness boundary. The current database schema version is `5`.

## Stored records

- runs, source workflow, compiled plan, inputs, output, mode, state, parent linkage, and repair source/root metadata
- task states, attempts, output, errors, disposition, source attempt, versioned fingerprints/digests, state delta, artifact manifest, and reuse decision
- effects, request/result/error, confirmation, and uncertainty
- approvals and resolutions
- checksummed checkpoints
- ordered audit and trace events
- provider sessions and tool calls
- namespaced long-term memory with optional expiry

Working memory is stored on the run and in checkpoints. Provider credentials are not stored. Other confidential content may be stored, including prompts, tool output, and remote artifacts.

Migration 5 adds `source_run_id`, `source_workflow_digest`, repair roots/reason/version, and task-boundary metadata used by repair. A repair transaction creates the run, materializes every reused task, creates pending fresh tasks, records provenance audit events, and writes its first checkpoint atomically. The source identifier is durable lineage rather than a foreign-key dependency, so source garbage collection does not delete a repair run.

Artifact manifests contain policy-resolved paths, byte sizes, and SHA-256 digests. The bytes remain in the configured durable workspace. Retain that workspace for as long as a task result may be repaired or audited.

## Migrations

The store reads SQLite `user_version` and applies forward migrations in order inside transactions. A database newer than the binary fails explicitly. Corrupt or incompatible serialized state also fails explicitly.

```text
agentctl db stats --db .agentctl/runtime.db --output json --color never
agentctl db migrate --db .agentctl/runtime.db --output json --color never
```

`db migrate` may write the database. Back up the database and its WAL state before an upgrade.

## Locking and permissions

The connection enables foreign keys, WAL mode, and a five-second busy timeout. Unix database files use mode `0600`. Windows relies on user-profile ACLs. Separate runs can share a database, but this is not a distributed lease and does not prevent two runs from changing the same external resource.

## Backups and recovery

Use an SQLite-aware online backup or stop writers before copying the database and WAL files. Restore the set consistently. Do not use ordinary file synchronization that can separate a database from uncheckpointed WAL content.

Delete old terminal history only after retention requirements are met:

```text
agentctl gc --db .agentctl/runtime.db --older-than-days 30 --output json --color never
```
