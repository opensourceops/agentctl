# Terminology

Use these terms consistently in workflows, documentation, issues, and reviews.

| Term | Meaning |
| --- | --- |
| Workflow | One versioned declarative YAML document and its compiled graph. |
| Task | One ordered graph node that invokes an action or agent. |
| Action | A typed runtime operation selected directly by a task. |
| Process mode | Bounded direct execution with the agentctl host identity. It is not a sandbox. |
| Container mode | Explicit process-action isolation in a local digest-pinned Docker/Podman image with the fixed agentctl container contract. |
| Isolation | An operating-system or container authority boundary. Policy and allowlists alone are not isolation. |
| Agent | A bounded provider-backed executor for one task. |
| Role | A named agent task with explicit provider, tool visibility, limits, and output contract. |
| Handoff | A typed durable task output consumed by an explicitly dependent role task. |
| Tool | A strict capability contract that an agent may request. |
| Provider | A native model API adapter behind provider-neutral contracts. |
| Stream event | One bounded, redacted provider progress record ordered within a task attempt. |
| Effect | A durably identified operation that observes or changes state outside pure computation. |
| Run | One durable execution, check, replay, repair, or fork record. |
| Attempt | One bounded execution attempt for a task. |
| Resume | Continue the same non-terminal run using durable progress. |
| Recorded replay | Create a new record from terminal stored results without calling executors. |
| Retry | Start another bounded attempt for a task after a definitive retry-safe failure. |
| Repair | Create a linked run from a terminal source, reuse compatible successful tasks outside selected boundaries, and execute the roots and descendants from a target workflow. |
| Fork | Create a child run that intentionally permits fresh effects. |
| Rerun | Informal term. Prefer repair for boundary selection or fork for a broader fresh execution. |
| Disposition | Whether a successful task was freshly `executed`, source-linked `reused`, or copied as `recorded` replay evidence. |
| Approval | A durable operator decision required before an effect may continue. |
| Checkpoint | A versioned, checksummed snapshot used for recovery. |
| Working memory | One run-local JSON object changed by explicit memory actions. |
| Long-term memory | Namespaced SQLite values shared across runs and managed by retention. |
| Pack | A local versioned manifest and reviewed reusable content with integrity checking. |

Do not use resume, replay, retry, repair, and fork interchangeably. None of them means exactly-once execution.
