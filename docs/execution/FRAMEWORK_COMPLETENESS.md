# Complete framework product surface

## Product contract

`agentctl` is a deterministic and declarative runtime for durable agentic
workflows. The compiler, graph, state machine, effect ledger, policy, and
versioned persistence remain authoritative. Models execute bounded typed tasks;
they do not own orchestration.

The complete supported surface contains:

- strict versioned YAML, typed inputs/outputs, constrained templates, typed
  conditions, and deterministic routing;
- sequential or bounded parallel DAG execution with stable commit order and
  explicit working-memory conflict rules;
- bounded foreach/matrix expansion, bounded loops, and namespaced
  sub-workflows;
- deterministic actions, bounded provider tasks, typed tools, structured
  handoffs, explicit compensation, and durable streaming events;
- local SQLite history, authenticated encryption for selected sensitive fields,
  content-addressed artifacts, checkpoints, audit, traces, and usage budgets;
- resume, operator reconciliation, terminal retry, selective repair, recorded
  replay, and explicit effectful fork;
- native provider adapters behind a provider-neutral interface;
- resilient MCP and A2A clients that never duplicate uncertain mutation;
- local/Git/immutable-archive packs with deterministic locking and optional
  established signature verification;
- reviewed packs, MCP, and a bounded process protocol as the extension model;
- exact, text, vector, and hybrid optional long-term-memory retrieval with
  explicit promotion;
- environment, mounted-file, and policy-gated process secret references whose
  values are never persisted;
- explicit network and process policies, honest isolation modes, and generic
  non-root/read-only container execution;
- human, final JSON, JSONL progress, inspection, export, migration, and
  administration commands;
- local, externally scheduled, CI, container, and embedded Rust operation.

## Determinism rules

1. Compilation fixes task identity, dependencies, expansion limits, schemas,
   policies, budgets, and commit order.
2. Parallel tasks read an immutable boundary snapshot. Their durable commits
   occur in compiled order.
3. Working-memory writes are declared. Conflicts fail before effect dispatch
   unless a versioned deterministic merge is explicit.
4. Dynamic children and loop iterations receive stable IDs and hard bounds.
5. Every external observation or mutation has a persisted identity before
   dispatch.
6. Uncertain mutating work is never silently repeated.
7. Provider sessions are task-local. Typed output, durable state, artifacts,
   and explicit handoff payloads are the only cross-task dataflow.
8. Recorded replay dispatches no provider, tool, process, network, filesystem,
   protocol, memory, or artifact-ingestion effect.

## Persistence boundaries

SQLite stores versioned metadata and encrypted sensitive fields. Artifact bytes
live in an immutable local content-addressed store rooted beside the database.
The database references blobs by digest and owns reachability and retention.
External artifact backends may implement the same interface later, but none is
required for the complete local product.

The database and artifact root are backed up together. A repair or retry run
materializes its own metadata references so source-row garbage collection does
not break it. Blob garbage collection removes only unreferenced content outside
the configured retention window.

## Recovery operations

- `resume` continues a nonterminal run and reuses confirmed effects.
- `reconcile` records operator-confirmed external reality without mutating the
  source effect.
- `retry` creates a new source-linked run for an identical workflow and reruns
  failed or explicitly selected boundaries.
- `repair` creates a new source-linked run for a changed compatible workflow.
- `replay` reconstructs a terminal run and recorded stream without fresh
  effects.
- `fork` creates a broad new execution with intentionally fresh effects.
- `compensate` performs explicitly declared best-effort reverse actions and
  never claims transactional rollback.

## Extension contract

There is no in-process native plugin ABI. Extensions use one of:

- reviewed declarative packs;
- MCP for remote tools;
- a versioned bounded process protocol for local executors.

Each executable extension declares schemas, capabilities, effects, limits, and
policy requirements. Process and MCP execution remain isolated effect
boundaries.

## Explicit non-goals

- hosted SaaS, public control plane, or public pack registry;
- chat application, free-form multi-agent conversation, or hidden model-owned
  routing;
- Kubernetes operator, runner fleet, public cloud scheduler, calendars, or
  event triggers;
- distributed scheduling, distributed leases, multi-host execution, or
  distributed storage;
- IDE or visual workflow editor;
- general configuration management;
- unbounded loops, unbounded model-controlled expansion, or arbitrary
  expression code;
- an unsafe in-process native plugin ABI;
- a claim that policy allowlists are an OS sandbox;
- a claim of exactly-once external mutation or transactional compensation.

External schedulers own triggers and overlap policy. Containers, VMs, platform
identities, and egress controls remain the strongest isolation boundary for
hostile workloads.
