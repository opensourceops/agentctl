# Reconcile an uncertain effect

An effect can be externally applied even when the process loses its acknowledgement. `agentctl` records that ambiguity as `started` or `uncertain` and refuses to guess. Reconciliation appends an operator conclusion; it never rewrites the source effect.

List a run and inspect one effect:

```text
agentctl effects --db .agentctl/runtime.db list RUN_ID --output json
agentctl effects --db .agentctl/runtime.db inspect EFFECT_ID --output json
```

Verify the external system using an independent identifier or query. Then record exactly one of these conclusions.

Not applied, so a fresh task attempt may dispatch it:

```text
agentctl effects --db .agentctl/runtime.db reconcile EFFECT_ID \
  --status not-applied \
  --actor operator-name \
  --reason "remote lookup returned no record" \
  --evidence-file evidence.json
```

Applied, with the externally confirmed result needed to resume:

```text
agentctl effects --db .agentctl/runtime.db reconcile EFFECT_ID \
  --status applied \
  --actor operator-name \
  --reason "remote lookup confirmed record ext-123" \
  --evidence-file evidence.json \
  --result-file result.json \
  --result-schema-file result.schema.json
```

Compensated, linked to a confirmed compensation effect:

```text
agentctl effects --db .agentctl/runtime.db reconcile EFFECT_ID \
  --status compensated \
  --actor operator-name \
  --reason "the created record was deleted" \
  --evidence-file evidence.json \
  --compensation-effect COMPENSATION_EFFECT_ID
```

When policy requires approval, add `--approved`. This is explicit non-interactive authorization and is stored with the operator identity and policy decision. It cannot override a policy denial.

Applied results are validated against a supplied JSON Schema. Model results must also decode as a provider response. Registered tools validate results against their output contracts, and extensions can register an operation-specific reconciliation hook.

The transition rules prevent contradictory history:

- `applied` may be superseded by another `applied` record or progress to `compensated`.
- `not_applied` may only be superseded by another `not_applied` record.
- `compensated` may only be superseded by another `compensated` record.
- compensation requires a different, same-run effect that is confirmed applied.

Every record includes actor, timestamp, reason, evidence, optional result and schema, authorization, trace, supersession, and compensation linkage. Audit and trace records are written in the same transaction.

For a non-terminal interrupted run, `resume` consumes an `applied` result without redispatch. A `not_applied` or `compensated` conclusion starts a fresh task attempt with a new effect identity. For terminal sources, create a compatible repair run. Unresolved non-idempotent effects are never silently repeated.
