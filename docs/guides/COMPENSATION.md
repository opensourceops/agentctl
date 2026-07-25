# Compensate applied effects

Compensation is an explicit best-effort workflow operation. It does not provide
transactional rollback or exactly-once mutation of an external system.

Declare one named effectful action on each task that can be compensated:

```yaml
spec:
  compensation:
    onFailure: manual
    approval: policy
  actions:
    provision:
      kind: builtin.write
    deprovision:
      kind: builtin.write
  tasks:
    - id: provision
      uses: action:provision
      with:
        path: artifacts/resource.txt
        content: provisioned
      compensate:
        uses: action:deprovision
        with:
          path: "${{ tasks.provision.output.path }}"
          content: compensated
        retry:
          maxAttempts: 2
          backoffMs: 100
```

`compensate.uses` must name an effectful action. Its input can use the original
run's durable `inputs`, task outputs, task variables, and working memory. The
compiler includes the declaration in task identity, validates every reference,
and copies it to bounded matrix, foreach, loop, and sub-workflow children.

## Plan and execute

```console
agentctl compensate SOURCE_RUN_ID --db .agentctl/runtime.db --plan
agentctl compensate SOURCE_RUN_ID --db .agentctl/runtime.db
agentctl compensate SOURCE_RUN_ID --task provision --db .agentctl/runtime.db
```

The source must be terminal. Planning examines its immutable effects:

- confirmed successful mutations and effects reconciled as `applied` are
  eligible;
- `not_applied` effects require no compensation;
- already `compensated` effects are never repeated;
- started or uncertain mutations block their task until an operator reconciles
  external reality.

Eligible tasks are emitted in reverse compiled graph order. The compensation
run is source-linked, sequential, and uses the original policy, actions,
protocol configuration, working-memory snapshot, and workspace boundary.
Compensation tasks continue after a sibling failure, so successful and failed
undo actions remain individually inspectable.

Each successful compensation effect appends a `compensated` reconciliation to
the source effect. The record links both run IDs, both task IDs, and the
compensation effect ID. Source effects and the terminal source run remain
unchanged.

## Approval and automatic execution

Manual execution is the default. Automatic failure handling must be explicit:

```yaml
spec:
  compensation:
    onFailure: automatic
    approval: always
```

`approval: policy` preserves the workflow policy. `always` requires durable
approval for the compensation run. `never` explicitly removes an approval
gate but does not bypass filesystem, process, network, provider, or tool
authorization.

Automatic compensation starts only after a failed terminal run. Cancellation
does not imply automatic compensation. A paused compensation run is resumed
with the ordinary approval and `resume` commands.

## Failure and recovery

Every compensation effect has the ordinary durable idempotency key and
at-most-once uncertainty behavior. Definitive retry-safe failures use the
declared bounded retry. An uncertain compensation effect is never sent again
until it is reconciled, including after its compensation run becomes terminal.
An `applied` reconciliation finalizes the linked source effect; `not_applied`
allows a new compensation attempt.

Running `compensate` again plans only source effects that do not already have a
confirmed compensation. This is the retry operation for a terminal partial
compensation run. Recorded replay of the compensation run dispatches no fresh
effects.

A compensated source task is not reusable by retry or selective repair.
Restart it explicitly, together with its required closure, because its former
external result has been intentionally undone.

Use `inspect` on both source and compensation run IDs. The source exposes
`effectReconciliations`; the compensation run exposes ordinary tasks, effects,
approvals, audit events, traces, and its `sourceRunId`.
