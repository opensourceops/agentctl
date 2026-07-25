# Tools and effects

A tool contract has a stable ID, description, input and output JSON Schema, capability, risk, effect class, idempotency, retry-safety flag, timeout, secret and network requirements, and approval mode. Inputs are validated before an executor is called; outputs are validated before entering messages or task state. Executor errors remain errors.

Effect classes are `pure`, `internal_state`, `observe`, `workspace_mutate`, `external_mutate`, `process_execution`, `network`, `model`, and `remote_agent`. Idempotency is `pure`, `idempotent`, `keyed`, `at_most_once`, or `unknown`. These values drive durable recovery and policy; model-provided MCP annotations never override them.

Built-in model-callable tools are workspace read, workspace write, and echo. Their declared kind must match compiler-enforced capability/effect/idempotency semantics; the packaged CLI registers each declared built-in executor. Function-call IDs, input/output digests, status, and effect correlation are stored per run.

Built-in actions are assign, assert, file read/write, direct process execution, run working-memory read/write, SQLite long-term-memory read/write, MCP call, and A2A delegation. File writes use a temporary file plus rename and return before/after/diff. Shell execution uses a direct executable and argv, never an implicit shell, clears inherited environment, applies allowlisted variables, and is not an OS sandbox.

Check mode executes pure/internal simulation and observation needed for dataflow but never filesystem, process, remote, or model mutation. Results say fully predictable, partially predictable, or requires execution; unknown external work is never reported as predicted.
