# Architecture diagrams

These diagrams explain implemented workflow API v1 behavior. Each diagram is
paired with text so the relationship is available when Mermaid cannot render.

## High-level system architecture

The CLI composes deterministic core contracts with persistence, runtime execution, concrete providers, protocols, and optional tracing.

```mermaid
flowchart LR
  accTitle: High-level agentctl system architecture
  accDescr: A workflow author uses the CLI, which joins deterministic core contracts to runtime providers, protocols, executors, tracing, and SQLite.
  User[Workflow author or operator] --> CLI[agentctl CLI]
  CLI --> Core[Core parser compiler policy state]
  CLI --> Runtime[Bounded deterministic runtime]
  Runtime --> Store[SQLite store]
  Runtime --> Providers[Native model providers]
  Runtime --> Protocols[MCP and A2A clients]
  Runtime --> Executors[Files processes and built-in tools]
  Runtime --> Traces[Audit events and optional OpenTelemetry]
  Core --> Runtime
```

The core owns the graph and rules. Concrete I/O stays at the runtime boundary. SQLite is local durable state, not a distributed service.

## Workflow compilation

Compilation turns one strict document into a versioned deterministic plan before execution begins.

```mermaid
flowchart LR
  accTitle: Workflow compilation stages
  accDescr: Versioned YAML passes through strict parsing, validation, graph construction, and digest calculation to become a compiled plan.
  Source[Versioned YAML] --> Parse[Strict parse and migration diagnostics]
  Parse --> Validate[References templates policy and capabilities]
  Validate --> Graph[Cycle check and declaration-order DAG]
  Graph --> Digest[Canonical workflow and plan digests]
  Digest --> Plan[Compiled plan]
```

Unknown fields, cycles, missing references, invalid templates, and unsupported provider capabilities fail before a run performs effects.

## Run lifecycle

A normal run moves through durable states and ends in one terminal result or a durable pause.

```mermaid
flowchart TD
  accTitle: Run lifecycle
  accDescr: A run creates durable records, executes ready tasks, persists results, and reaches success, approval, failure, or cancellation.
  Create[Create run and task records] --> Ready[Select stable ready batch]
  Ready --> Execute[Execute independent tasks]
  Execute --> Persist[Commit ordered batch checkpoint and audit]
  Persist --> More{More ready tasks?}
  More -->|Yes| Ready
  More -->|No| Success[Succeeded]
  Execute --> Approval[Pending approval]
  Execute --> Failure[Failed or cancelled]
```

The scheduler selects up to `maxConcurrency` ready tasks in declaration order.
Every task reads a durable isolated snapshot. Results commit atomically in plan
order. A pending approval is non-terminal and can later resume.

## Runtime state machine

Run and task transitions are validated instead of being inferred from missing records.

```mermaid
stateDiagram-v2
  accTitle: Runtime state machine
  accDescr: Pending runs become running, can wait for approval, and terminate as succeeded, failed, or cancelled.
  [*] --> Pending
  Pending --> Running
  Running --> WaitingApproval
  WaitingApproval --> Running: approval resolved and resume
  Running --> Succeeded
  Running --> Failed
  Running --> Cancelled
  Pending --> Cancelled
  Succeeded --> [*]
  Failed --> [*]
  Cancelled --> [*]
```

Invalid transitions fail explicitly. Replay requires a terminal source; resume requires a safe non-terminal source.

## Effect recording

Every non-pure operation is requested durably before an executor starts.

```mermaid
sequenceDiagram
  accTitle: Effect recording sequence
  accDescr: The runtime records an effect before dispatch and then persists either its confirmed result or an uncertain state.
  participant R as Runtime
  participant S as SQLite store
  participant E as Effect executor
  R->>S: Persist requested effect and stable identity
  R->>S: Mark effect started
  R->>E: Dispatch bounded operation
  alt confirmed result
    E-->>R: Result
    R->>S: Commit confirmed result and task evidence
  else dispatch outcome ambiguous
    R->>S: Mark effect uncertain
  end
```

A request-before-start ledger supports reuse of confirmed results. It does not prove exactly-once behavior in an external system.

## Resume flow

Resume continues the same non-terminal run only when durable evidence makes continuation safe.

```mermaid
flowchart TD
  accTitle: Resume flow
  accDescr: Resume loads the same run, stops for uncertain effects, reuses confirmed results, and continues safe pending work.
  Load[Load same run and checkpoint] --> Scan[Inspect tasks approvals and effects]
  Scan --> Uncertain{Started unconfirmed effect?}
  Uncertain -->|Yes| Stop[Stop for operator reconciliation]
  Uncertain -->|No| Confirmed{Confirmed prior effect?}
  Confirmed -->|Yes| Reuse[Reuse recorded result]
  Confirmed -->|No| Continue[Execute next requested work]
  Reuse --> Continue
```

Resume preserves the run identity and progress. It never silently repeats an uncertain external effect.

## Recorded replay flow

Recorded replay constructs a new linked record entirely from terminal stored evidence.

```mermaid
flowchart LR
  accTitle: Recorded replay flow
  accDescr: Replay copies terminal stored evidence into a linked replay run without calling providers, tools, files, processes, or networks.
  Terminal[Terminal source run] --> Validate[Validate all source tasks are terminal]
  Validate --> Copy[Copy recorded task outputs effects and tool calls]
  Copy --> Replay[Create replay-mode run linked to source]
  Replay --> Result[Return recorded outcome]
  Executors[Provider tool file process and network executors] -. not called .-> Replay
```

Replay reports historical truth. It does not observe current files, rerun verification, or contact a provider.

## Selective repair flow

Selective repair is a new source-linked run. It is distinct from both effect-free recorded replay and broad fresh fork execution.

```mermaid
flowchart LR
  accTitle: Selective repair flow
  accDescr: Repair verifies successful upstream task boundaries, materializes compatible outputs and state, and executes selected roots and descendants from a target workflow.
  Source[Terminal source run] --> Plan[Effect-free compatibility plan]
  Target[Target workflow] --> Plan
  Plan --> Reuse[Materialize compatible upstream tasks]
  Plan --> Fresh[Execute roots and descendants]
  Reuse --> Boundary[Reconstructed task-boundary state]
  Boundary --> Fresh
  Fresh --> Repair[New repair run and trace]
  Source -. remains immutable .-> Repair
```

The detailed failed-run, plan, reuse, invalidation, lineage, and effect-safety diagrams are in [Repair a failed workflow](../guides/repair-a-failed-workflow.md).

## Fork or rerun flow

Fork makes fresh execution an explicit choice instead of overloading replay.

```mermaid
flowchart LR
  accTitle: Fork or rerun flow
  accDescr: Fork loads a prior declaration into a child run, resets working memory, and executes fresh effects for an independent outcome.
  Source[Prior run] --> Load[Load source workflow plan and inputs]
  Load --> Child[Create child run with parent link]
  Child --> Reset[Use declared initial working memory]
  Reset --> Execute[Execute tasks with fresh effects]
  Execute --> Outcome[Independent outcome]
```

Fork may repeat external mutations and may produce a different result. Reconcile uncertain source effects before forking.

## Approval lifecycle

Policy can stop an effect before dispatch and require an operator-controlled decision.

```mermaid
sequenceDiagram
  accTitle: Approval lifecycle
  accDescr: Policy requires approval, the runtime persists and reports it, an operator resolves it, and the same run resumes.
  participant R as Runtime
  participant P as Policy engine
  participant S as SQLite store
  participant O as Operator
  R->>P: Evaluate capability resource and risk
  P-->>R: Approval required
  R->>S: Persist redacted approval request
  R-->>O: Exit 3 with run and trace IDs
  O->>S: Approve or reject with actor and reason
  O->>R: Resume same run
  R->>S: Read resolution and continue or fail
```

Non-interactive execution never waits on stdin or auto-approves. The platform authenticates the operator.

## Container deployment

The production image is a non-root CLI with four explicit host-managed mounts.

```mermaid
flowchart LR
  accTitle: Container deployment contract
  accDescr: A non-root agentctl container reads config and workspace mounts, writes state and artifacts, receives secret references, and returns JSON and an exit code.
  Config[Read-only /config] --> Container[Distroless agentctl UID 65532]
  Workspace[Usually read-only /workspace] --> Container
  Container --> State[Writable /state SQLite]
  Container --> Artifacts[Writable /artifacts]
  Secrets[Environment secret references] --> Container
  Container --> Output[One JSON result and process exit code]
```

The root filesystem can remain read-only. State must persist for inspection, approval, resume, and replay.

## CI/CD execution

CI remains the outer scheduler and artifact system.

```mermaid
flowchart TD
  accTitle: CI and CD execution
  accDescr: CI prepares mounts, runs the generic container step, interprets JSON and exit status, collects evidence, and handles approval through an operator job.
  Checkout[CI checks out workflow and source] --> Prepare[Prepare config state workspace and artifacts]
  Prepare --> Run[Run generic agentctl OCI step]
  Run --> Code[Interpret stable exit code]
  Run --> Json[Parse final JSON envelope]
  Run --> Collect[Collect protected state and declared artifacts]
  Code --> Approval{Exit 3?}
  Approval -->|Yes| Operator[Operator-controlled resolution job]
  Operator --> Resume[Resume with retained state]
```

GitHub Actions, GitLab CI, Jenkins, Harness CI, and Kubernetes use the same mount and process contract.

## Provider and tool interaction

The runtime, not the model, owns tool visibility, policy, validation, and effect recording.

```mermaid
sequenceDiagram
  accTitle: Provider and tool interaction
  accDescr: The runtime maps neutral messages through a native provider, validates model tool calls, authorizes execution, and returns correlated results.
  participant R as Runtime agent loop
  participant P as Native provider
  participant M as Model
  participant T as Tool executor
  R->>P: Provider-neutral messages and strict tools
  P->>M: Native request
  M-->>P: Tool call or final content
  P-->>R: Neutral response and continuation
  R->>R: Validate schema policy approval and limits
  R->>T: Execute authorized tool with effect identity
  T-->>R: Validated output
  R->>P: Correlated tool result and continuation
```

Provider-native types stop at the adapter. Model output cannot change policy or tool metadata.

## MCP and A2A boundaries

Both protocols are remote effects with pinned versions, explicit hosts, environment-backed headers, and conservative ambiguity handling.

```mermaid
flowchart LR
  accTitle: MCP and A2A boundaries
  accDescr: Runtime policy and the effect ledger mediate communication with untrusted remote MCP tool servers and A2A agents.
  Workflow[Compiled workflow] --> Runtime[Runtime policy and effect ledger]
  Runtime --> MCP[MCP 2025-11-25 Streamable HTTP]
  Runtime --> A2A[A2A 1.0 Agent Card and JSON-RPC]
  MCP --> McpPeer[Untrusted remote tool server]
  A2A --> A2aPeer[Untrusted remote agent]
  McpPeer --> Runtime
  A2aPeer --> Runtime
```

MCP annotations and A2A cards are untrusted metadata. The clients do not automatically reconnect or resubmit after an ambiguous operation.

## Crate dependency map

The dependency direction keeps deterministic contracts independent from adapters.

```mermaid
flowchart TD
  accTitle: Crate dependency map
  accDescr: CLI, runtime, provider, protocol, store, observability, and xtask crates depend inward on deterministic core contracts.
  CLI[agentctl-cli] --> Runtime[agentctl-runtime]
  CLI --> Providers[agentctl-providers]
  CLI --> Protocols[agentctl-protocols]
  CLI --> Store[agentctl-store]
  Runtime --> Core[agentctl-core]
  Runtime --> Store
  Runtime --> Observability[agentctl-observability]
  Providers --> Core
  Protocols --> Core
  Protocols --> Runtime
  Store --> Core
  Observability --> Core
  Xtask[xtask] --> CLI
```

`agentctl-core` has no dependency on HTTP, SQLite, CLI parsing, or concrete executor types. `xtask` drives the built CLI for generation and acceptance.
