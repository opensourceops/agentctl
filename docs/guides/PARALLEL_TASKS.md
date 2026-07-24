# Deterministic parallel tasks

Set `spec.runtime.maxConcurrency` between `1` and `64`. The default remains
`1`, so existing workflows keep sequential behavior. A value greater than one
allows independent ready tasks to execute in bounded batches.

```yaml
spec:
  runtime:
    maxConcurrency: 2
  actions:
    remember:
      kind: builtin.memory.write
  tasks:
    - id: left
      uses: action:remember
      with: { key: left, value: one }
    - id: right
      uses: action:remember
      with: { key: right, value: two }
```

## Deterministic boundary

The compiler fixes task order with the declaration-order topological plan. The
runtime selects ready tasks in that order up to the concurrency limit. Every
task in a batch reads an immutable working-memory snapshot. That snapshot is
encrypted when state encryption is enabled and retained while a task is
running or waiting for approval, so resume does not substitute newer sibling
state after a crash.

Task bodies may finish in any order. Successful outputs, state deltas, artifact
references, failures, retry states, audit events, the final working-memory
value, and a checkpoint commit in plan order in one SQLite transaction. A
failed transaction exposes none of the batch results.

Effects keep their existing stable identity of run, task, attempt, ordinal,
operation, and input digest. Parallel execution does not combine provider
sessions or tool-call histories. Recorded replay dispatches no effects.

## Working-memory writes

A literal `builtin.memory.write` key is inferred into the compiled task's
`memoryWrites` set. A templated key must declare every possible key:

```yaml
- id: selected-write
  uses: action:remember
  memoryWrites: [left, right]
  with:
    key: "${{ inputs.selected }}"
    value: kept
```

For `maxConcurrency` greater than one, unordered tasks with overlapping write
sets fail compilation. Add a `needs` edge or use disjoint keys. There is no
implicit last-writer-wins rule and no merge strategy in this format. The
runtime also verifies the rendered key before creating an effect and verifies
the completed delta before commit.

The write set covers run working memory only. Tasks that can touch the same
file, remote object, process resource, or other external target must be ordered
with `needs` unless that external system provides the required concurrency
control.

## Failures, approvals, and cancellation

A stop-on-failure task does not abandon already launched siblings. The runtime
waits for their bounded execution, commits successful sibling results with the
failure, cancels remaining tasks and pending approvals, and marks the run
failed atomically. With `failure: continue`, independent branches continue and
descendants of the failed task skip normally.

Approval-gated tasks may pause beside completed siblings. Their immutable
execution snapshot and effect request remain durable. Resume continues only
approved tasks and never repeats a confirmed effect. Cancellation propagates
through the shared cancellation token and durably cancels every non-terminal
task.

Failed-only retry and selective repair use the ordinary graph closure. Proven
successful parallel siblings are reusable; selected failed or changed branches
receive fresh attempts. Offline replay applies recorded state deltas in
compiled order.

See [`examples/v1/parallel.yaml`](../../examples/v1/parallel.yaml) for a
credential-free executable example.
