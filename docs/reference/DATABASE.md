# Runtime database and migrations

The local SQLite database and its sibling artifact root are history and part of the correctness boundary. The current database schema version is `8`.

## Stored records

- runs, source workflow, compiled plan, inputs, output, mode, state, parent linkage, and repair source/root metadata
- task states, attempts, output, errors, disposition, source attempt, versioned fingerprints/digests, state delta, artifact manifest, and reuse decision
- effects, request/result/error, confirmation, and uncertainty
- immutable effect reconciliation history, operator authorization, evidence, validated results, supersession, and compensation linkage
- approvals and resolutions
- checksummed checkpoints
- ordered audit and trace events
- provider sessions and tool calls
- namespaced long-term memory with optional expiry
- content-addressed blob metadata, logical run/task references, provenance, verification time, and bounded ingestion leases
- legacy-run upgrade analysis and the exact task metadata applied by each upgrade

Working memory is stored on the run and in checkpoints. Provider credentials are not stored. Other confidential content may be stored, including prompts, tool output, and remote artifacts.

Migration 5 adds `source_run_id`, `source_workflow_digest`, repair roots/reason/version, and task-boundary metadata used by repair. Migration 6 adds artifact blob, reference, and ingestion-lease tables. Migration 7 records transactional legacy-run upgrades. Migration 8 adds immutable effect reconciliation records. A repair transaction creates the run, materializes every reused task and artifact reference, creates pending fresh tasks, records provenance audit events, and writes its first checkpoint atomically. The source identifier is durable lineage rather than a foreign-key dependency, so source garbage collection does not delete a repair run.

Artifact manifests contain logical path/name, media type, byte size, SHA-256 digest, and CAS-relative path. Blob bytes live under `<database-parent>/artifacts/sha256/`; identical content is stored once. A completed repair/replay receives its own references, so source-row and workspace deletion do not break it.

## Migrations

The store reads SQLite `user_version` and applies forward migrations in order inside transactions. A database newer than the binary fails explicitly. Corrupt or incompatible serialized state also fails explicitly.

```text
agentctl db stats --db .agentctl/runtime.db --output json --color never
agentctl db migrate --db .agentctl/runtime.db --output json --color never
agentctl runs --db .agentctl/runtime.db analyze RUN_ID --output json
agentctl runs --db .agentctl/runtime.db upgrade RUN_ID --dry-run --output json
agentctl runs --db .agentctl/runtime.db upgrade RUN_ID --output json
agentctl effects --db .agentctl/runtime.db list RUN_ID --output json
agentctl artifacts --db .agentctl/runtime.db list --run RUN_ID --output json
agentctl artifacts --db .agentctl/runtime.db verify --all --output json
agentctl artifacts --db .agentctl/runtime.db export SHA256_DIGEST ./report.bin
agentctl artifacts --db .agentctl/runtime.db gc --older-than-days 30 --dry-run
```

`db migrate` may write the database. Back up the database and its WAL state before an upgrade.

## Locking and permissions

The connection enables foreign keys, WAL mode, and a five-second busy timeout. Unix database files use mode `0600`. Windows relies on user-profile ACLs. Artifact ingestion and GC use an advisory cross-process lock plus SQLite leases; this protects the local store but is not a distributed lease. Separate runs can share a database but can still change the same external resource.

## Backups and recovery

Use an SQLite-aware online backup or stop writers before copying the database, WAL files, and sibling `artifacts/` directory. Restore the set consistently. Do not use ordinary file synchronization that can separate SQLite from uncheckpointed WAL content or the artifact bytes referenced by it.

Delete old terminal history only after retention requirements are met:

```text
agentctl gc --db .agentctl/runtime.db --older-than-days 30 --output json --color never
agentctl artifacts --db .agentctl/runtime.db gc --older-than-days 30 --output json --color never
```
