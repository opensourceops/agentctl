# Decision index

| ADR | Decision | Status | Consequence |
| --- | --- | --- | --- |
| [0001](../adr/0001-deterministic-core-explicit-effects.md) | Deterministic core, explicit effects | accepted | Models never own graph/policy/history; uncertain effects stop recovery. |
| [0002](../adr/0002-versioned-strict-workflow-envelope.md) | Versioned strict envelope | accepted | Generated schema, strict fields, explicit migration. |
| [0003](../adr/0003-sqlite-history-and-conservative-recovery.md) | SQLite history, conservative recovery | accepted | Resume reuses confirmed effects; replay performs none; fork is fresh. |
| [0004](../adr/0004-native-provider-adapters.md) | Native providers behind neutral contracts | accepted | Native request/response tests and early capability rejection. |
| [0005](../adr/0005-narrow-v1-scheduling-and-extensions.md) | Narrow v1 scheduling and extensions | superseded for scheduling | Dynamic constructs remain separate decisions; ADR 0008 replaces the sequential-only rule. |
| [0006](../adr/0006-schedulable-runtime-and-noninteractive-contract.md) | Schedulable runtime, durable non-interactive pause | accepted | External platforms schedule; CLI never prompts or auto-approves in CI. |
| [0007](../adr/0007-generic-oci-step-contract.md) | Generic OCI step contract | accepted | Non-root/read-only image; mounted config/workspace/state/artifacts. |
| [0008](../adr/0008-deterministic-parallel-batches.md) | Deterministic parallel batches | accepted | Bounded overlap, isolated snapshots, declared writes, and atomic plan-order commits. |
| [0009](../adr/0009-bounded-static-task-expansion.md) | Bounded static task expansion | accepted | Stable child tasks and ordered aggregates prevent model-controlled graph growth. |
| [0010](../adr/0010-typed-routing-and-durable-decisions.md) | Typed routing and durable decisions | accepted | Pure enumerated routers and hashed condition contexts make branching inspectable and replayable. |

These decisions resolve the researched patterns in [LANDSCAPE.md](../research/LANDSCAPE.md). No unsafe code or distributed control plane ADR is required because neither exists.
