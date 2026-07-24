# Upgrade a legacy run for selective repair

Runs written before database schema 5 can contain successful outputs without the fingerprints, state deltas, and artifact identities required for safe reuse. `agentctl` does not invent those fields. It analyzes retained workflow, plan, effect, output, and checksummed checkpoint records and reports which task boundaries are provable.

First migrate the database and run a read-only analysis:

```text
agentctl db --db .agentctl/runtime.db migrate
agentctl runs --db .agentctl/runtime.db analyze RUN_ID --output json
```

`upgradeableTasks` lists successful tasks whose complete metadata can be derived. Each task includes field-level provenance. `unavailableTasks` lists boundaries with missing or contradictory proof. `recommendedRepairRoots` is the conservative earliest safe suffix: every later task is covered unless dependency closure already covers it.

`analyze` and `upgrade --dry-run` do not write:

```text
agentctl runs --db .agentctl/runtime.db upgrade RUN_ID --dry-run --output json
```

Apply the proven subset explicitly:

```text
agentctl runs --db .agentctl/runtime.db upgrade RUN_ID --output json
```

The upgrade imports provable regular-file artifacts into the sibling content-addressed store, verifies their recorded identity, updates legacy task metadata, appends an upgrade record and audit event, and checkpoints the run in one SQLite transaction. It preserves the source output, effect, checkpoint, and workflow records. A failed task update rolls back all metadata and references from that upgrade. Artifact ingestion leases may remain temporarily and are reclaimed by normal artifact GC.

Use the returned roots with the corrected workflow:

```text
agentctl repair repaired.workflow.yaml RUN_ID \
  --from SAFE_ROOT \
  --plan \
  --db .agentctl/runtime.db
```

If several roots are returned, pass `--from` once for each root. A task that remains unprovable is executed again; compatible proven predecessors remain reusable. After a successful upgrade and repair, content-addressed artifacts remain usable if the original workspace file is deleted.

Back up SQLite, its WAL state, and the sibling `artifacts/` directory together before migration. `cargo xtask migration-verify` exercises every retained database schema fixture.
