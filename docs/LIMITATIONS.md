# Limitations and roadmap classification

This classification is part of the product contract. A deferred feature is not a current capability, but its absence is not automatically a release blocker for the local, externally scheduled, and generic OCI-step journeys.

## Release blockers

No known P0/P1 implementation defect remains for the stated local, scheduled, and OCI journeys. The local container build now has a secure optional CA secret path, and the current image passed OCI acceptance, Trivy 0.72.0, and CycloneDX validation. The remaining RC gate is external evidence: the new Linux x64, macOS arm64, Windows x64, container, security, package, and SBOM workflows are configured and locally linted but have not been pushed or dispatched. The recommendation is **Ready for hosted RC validation**, not an already validated RC or stable v1.0.

## Required hardening completed for this release

- Provider-specific options are allowlisted, type-checked, included in plan capability negotiation, and either mapped or rejected. Streaming and programmatic tool calling are rejected rather than ignored.
- Tool input/output schemas are strict; built-in tool kinds have compiler-checked capability/effect/idempotency contracts.
- Provider calls, function-call IDs/results, continuations, effects, checkpoints, audit events, and redacted trace events are durable and publicly inspectable.
- Timeout/transport ambiguity is not automatically retried; confirmed effects survive resume; call IDs are scoped by run; missing credentials fail before run/database creation.
- Non-interactive approvals durably pause, signals cancel safely, JSON errors include available run/trace correlation, and SQLite uses WAL plus a bounded lock wait.
- The packaged CLI, clean-directory quickstart, cron-like empty environment, and non-root/read-only OCI contract have executable acceptance coverage.
- Successful bounded file outputs are atomically ingested into a local immutable content-addressed store with durable references, verification/export commands, lease-safe reachability GC, interrupted-GC recovery, and local/OCI acceptance coverage.
- Identified confidential JSON and text fields can be transactionally migrated to versioned AES-256-GCM envelopes, rotated through environment key references, inventoried without content disclosure, and fail closed on missing/wrong keys, tampering, plaintext writes, or stale-key writes.
- Shell execution and acceptance/container helpers use bounded concurrent capture. Output overflow terminates/reaps the child with a structured secret-safe error; timeouts and cancellation retain durable uncertain-effect semantics.
- Hosted workflows use least privilege, full-SHA action pins with version comments, complete-history/tree Gitleaks, deterministic fake-secret detection, dependency/image scans, and required production/image CycloneDX artifacts with digests.

## Post-v1 features

These are useful extensions but are not required by the product thesis. They need new deterministic state and compatibility contracts before implementation:

- model token streaming into CLI/workflow state;
- opt-in MCP reconnection and A2A resubmission with explicit remote reconciliation;
- pack dependency resolution, pack lockfiles, remote fetching, publisher signatures, and a versioned plugin ABI;
- vector memory;
- external secret-manager adapters beyond environment, mounted-file, and policy-gated process references;
- reliable monetary cost enforcement when providers expose sufficient authoritative metadata.

## Explicit non-goals

- Event triggers and calendars: external schedulers trigger `agentctl`.
- MongoDB migration, distributed scheduling, multi-host execution, and distributed storage: the correctness boundary is one local process and SQLite database.
- An in-process OS sandbox or stronger network isolation: allowlists are defense in depth, while containers/VMs, identities, egress policy, and platform sandboxes own isolation.
- Free-form multi-agent conversation control flow: the compiled workflow remains authoritative.

## Current operational limits

- The document API is `v1alpha1`; pin the binary/image version and validate before upgrading.
- Parallel scheduling is local to one run and process, bounded at 64 tasks, and defaults to sequential execution. Working-memory conflicts fail compilation, but tasks that target the same external resource still require explicit `needs` ordering or that system's concurrency controls. Separate runs also require external overlap controls when effects must not overlap.
- Foreach and matrix expansion accepts only static workflow values, requires
  `maxItems`, and is capped at 256 children. Runtime or model-controlled graph
  growth is not supported.
- Conditions support typed paths, equality, inequality, numeric ordering, and
  `not`; routers support exact typed selectors and enumerated destinations.
  Arbitrary expressions, implicit dependencies, and model-owned hidden routing
  are rejected.
- Loops are sequential, require a maximum from 1 through 64, and compile all
  iteration boundaries before execution. Runtime or model-controlled graph
  growth and unbounded loops are rejected.
- Sub-workflows are compile-time namespaced graphs with semantic versions and
  typed input/output boundaries. Definitions inherit the caller's policy and
  providers and cannot request independent authority.
- Compensation is explicit best-effort inverse execution. It runs as a
  source-linked sequential workflow, skips effects already reconciled as
  compensated, and never claims transactional rollback or exactly-once
  external mutation.
- Structured collaboration is an explicit graph of bounded role tasks and
  typed handoff tasks. A hidden `team:` conversation scheduler is rejected.
  Role tool visibility is fixed by each agent definition; workflow policy
  remains authoritative for every role.
- SQLite is local durable state, not a secret vault or distributed lease service. Persist `/state` across container invocations and back it up according to the workflow's recovery needs.
- State encryption is explicit and selected-field only. Before it is enabled, the database is plaintext. It does not encrypt artifact bytes or operational metadata, and it cannot retroactively protect old backups or snapshots. Preserve the current referenced key with encrypted backups.
- Filesystem/process/network allowlists are not an OS sandbox. Run untrusted workflows in a restricted container/VM with least-privilege credentials and egress.
- At-most-once model/remote calls can become uncertain in the dispatch/acknowledgement window. Inspect and reconcile externally; use `fork` only when fresh effects are knowingly acceptable.
- Successful tasks from databases created before schema 5 require explicit `runs analyze`/`runs upgrade`. Only provable metadata is imported; unprovable boundaries are returned as conservative safe repair roots.
- Automatic artifact ingestion covers regular files up to 16 MiB reported by successful built-in workspace-mutation results. Larger outputs and artifacts produced only by opaque external effects require an explicit bounded import/export integration. The local CAS must be backed up with SQLite; missing or corrupt blob bytes block repair before run creation and report the expected artifact identity.
- An applied non-idempotent mutation in a repair closure remains blocked from duplicate execution unless a confirmed compensation is linked. Reconciliation supports immutable `applied`, `not_applied`, and `compensated` records, validated results, policy authorization, and operation-specific verification hooks; it does not provide exactly-once delivery.
- Terminal retry requires an identical workflow digest and a terminal source. It creates a new source-linked run, reuses only proven compatible successful boundaries, and freshly executes the selected closure. Use repair for a changed definition, resume for a non-terminal run, replay for effect-free reconstruction, and fork for knowingly broad fresh execution.
- Tool-using OpenAI/Azure agents require stored-response continuation. `store: false` is rejected until stateless response-item replay is implemented.
- Anthropic, Google, Azure OpenAI, MCP, and A2A are native and mock-tested in this release, not live-tested. Only the OpenAI GPT-5.6 tool path has live end-to-end evidence.
- The current local OCI runtime, vulnerability-scan, and SBOM evidence is Linux arm64. Linux x64 is configured in the unpushed Ubuntu workflow but has not executed.
- GitHub runner availability, organization action policy, branch protection, and required-check configuration are repository-owner operations and cannot be proven by repository-local lint.
