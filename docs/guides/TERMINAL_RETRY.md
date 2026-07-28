# Retry a terminal workflow

Use terminal retry when a run is terminal, its workflow definition is unchanged, and failed or selected task boundaries should execute again. Retry creates a new source-linked run. It never reopens or mutates the source.

## Plan failed boundaries

Inspect the source and understand any failed, started, or uncertain effects:

```console
agentctl inspect SOURCE_RUN_ID --db .agentctl/runtime.db --output json
agentctl effects --db .agentctl/runtime.db list SOURCE_RUN_ID --output json
```

Create an effect-free plan for every failed task and its descendants:

```console
agentctl retry workflow.yaml SOURCE_RUN_ID \
  --failed \
  --plan \
  --db .agentctl/runtime.db \
  --output json \
  --color never
```

The plan reports retry roots, reusable successful tasks, tasks that will run, possible effects and approvals, compatibility blocks, and warnings. A compatible plan exits `0`; a blocked but parseable plan exits `3`. Retry requires the target workflow digest to equal the source digest exactly. Use repair if the workflow changed.

Select one or more explicit roots instead of all failed tasks when needed:

```console
agentctl retry workflow.yaml SOURCE_RUN_ID \
  --from publish \
  --from verify \
  --plan
```

`--failed` and `--from` are mutually exclusive. Selecting a task that already succeeded requires `--restart-successful`, because its closure will execute fresh effects.

## Execute the retry

```console
agentctl retry workflow.yaml SOURCE_RUN_ID \
  --failed \
  --reason "transient dependency recovered" \
  --db .agentctl/runtime.db \
  --output json \
  --color never
```

The result contains a new run ID, source run ID, retry roots, reused tasks, freshly executed tasks, final state, artifacts, and workflow output. `inspect` identifies the new run mode as `retry`, preserves the source lineage and reason, and marks each task `reused` or `executed`.

Successful tasks outside the retry closure are reused only after their definition, dependencies, resolved inputs, output contract and digest, committed state delta, artifact integrity, and effect certainty pass the same boundary checks used by repair. Reused tasks dispatch no provider, tool, process, network, or filesystem operation. Roots and descendants start fresh task and effect attempts.

## Resolve uncertain effects

Retry will not guess whether an ambiguous external mutation happened. Inspect the effect and append an authorized reconciliation only after checking external reality:

```console
agentctl effects --db .agentctl/runtime.db inspect EFFECT_ID
agentctl effects --db .agentctl/runtime.db reconcile EFFECT_ID \
  --status not-applied \
  --reason "remote system confirms no mutation" \
  --actor operator-name
```

Then create a new plan. There is no force flag and no exactly-once claim. See [Effect reconciliation](EFFECT_RECONCILIATION.md).

## Choose the right recovery operation

| Operation | Use it when | Identity and effects |
| --- | --- | --- |
| `resume` | The source is paused or otherwise non-terminal. | Continues the same run and definition. |
| `retry` | The source is terminal and the workflow is identical. | New linked run; compatible success is reused; selected closure is fresh. |
| `repair` | The source is terminal and the workflow was corrected. | New linked run under compatibility checks; selected closure is fresh. |
| `replay` | Stored results must be reconstructed or audited. | New recorded run with zero fresh effects. |
| `fork` | A broad fresh execution is knowingly intended. | New linked run that permits fresh effects throughout. |

After a successful retry, recorded replay remains offline:

```console
env -u OPENAI_API_KEY agentctl replay RETRY_RUN_ID \
  --db .agentctl/runtime.db \
  --output json \
  --color never
```
