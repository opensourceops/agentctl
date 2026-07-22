# Terminology

Use these terms consistently in workflows, documentation, issues, and reviews.

| Term | Meaning |
| --- | --- |
| Workflow | One versioned declarative YAML document and its compiled graph. |
| Task | One ordered graph node that invokes an action or agent. |
| Action | A typed runtime operation selected directly by a task. |
| Agent | A bounded provider-backed executor for one task. |
| Tool | A strict capability contract that an agent may request. |
| Provider | A native model API adapter behind provider-neutral contracts. |
| Effect | A durably identified operation that observes or changes state outside pure computation. |
| Run | One durable execution, check, replay, or fork record. |
| Attempt | One bounded execution attempt for a task. |
| Resume | Continue the same non-terminal run using durable progress. |
| Recorded replay | Create a new record from terminal stored results without calling executors. |
| Retry | Start another bounded attempt for a task after a definitive retry-safe failure. |
| Fork | Create a child run that intentionally permits fresh effects. |
| Rerun | Informal term. Prefer fork when referring to the supported fresh-run operation. |
| Approval | A durable operator decision required before an effect may continue. |
| Checkpoint | A versioned, checksummed snapshot used for recovery. |
| Working memory | One run-local JSON object changed by explicit memory actions. |
| Long-term memory | Namespaced SQLite values shared across runs and managed by retention. |
| Pack | A local versioned manifest and reviewed reusable content with integrity checking. |

Do not use resume, replay, retry, and fork interchangeably. None of them means exactly-once execution.
