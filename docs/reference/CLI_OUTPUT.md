# CLI output and exit codes

The [generated CLI reference](../generated/CLI.md) is produced from Clap help by `cargo xtask generate`. This page explains the stable process contract used by scripts and CI.

## Output modes

Human mode is the default and may use color on a terminal. JSON mode emits one final object and never includes ANSI color:

```text
agentctl run workflow.yaml --output json --color never
```

The envelope version is `agentctl.dev/cli/v1` and includes `kind`, `ok`, `data`, and `diagnostics`. A successful run includes run ID, trace ID, terminal state, and declared output. A failure writes a versioned error envelope to stderr, with run and trace correlation when a run exists.

JSONL mode emits one versioned envelope per durable provider stream event,
followed by one final outcome envelope:

```text
agentctl run workflow.yaml --output jsonl --color never
```

Human mode writes provider progress to stderr. JSON mode never renders progress
and remains exactly one final document. All modes persist the same bounded
records for `agentctl inspect`.

## Exit codes

| Code | Category | Pipeline response |
| --- | --- | --- |
| `0` | success | Collect outputs and artifacts. |
| `2` | usage or validation | Correct arguments, YAML, references, templates, or capabilities. |
| `3` | policy, approval, or blocked repair plan | Inspect denial/compatibility evidence or retain state for operator approval. |
| `4` | run failure | Inspect the failed task and effect history. |
| `5` | persistence | Check database compatibility, permissions, corruption, and locking. |
| `6` | provider or protocol | Diagnose authentication, network, native API, MCP, or A2A evidence. |
| `130` | cancellation | Inspect the run before deciding whether resume is safe. |

Do not automatically retry every nonzero code. A provider, protocol, process, or tool operation may be uncertain after dispatch.

`repair --plan` returns kind `RepairPlan`. A compatible plan exits `0`; a blocked plan exits `3` while remaining a successful, parseable machine envelope with `compatible: false` and `blockedReuse` explanations. Repair execution returns kind `RepairOutcome` with new/source run IDs, trace ID, state, reused tasks, executed tasks, and output. `inspect` exposes the complete run and task lineage.

## Example

```text
agentctl check examples/v1/hello.yaml --output json --color never
agentctl run examples/v1/hello.yaml --db /tmp/hello.db --output json --color never
```

These commands need no credential. `check` writes no runtime database; `run` writes `/tmp/hello.db`.
