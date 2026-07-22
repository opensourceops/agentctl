# Limitations and roadmap classification

This classification is part of the product contract. A deferred feature is not a current capability, but its absence is not automatically a release blocker for the local, externally scheduled, and generic OCI-step journeys.

## Release blockers

No known P0/P1 implementation defect remains for the stated local, scheduled, and OCI journeys after the independent review. The exact final live OpenAI database passed another credential-free replay with identical output, zero fresh effects/tool calls/provider sessions, and explicit source-effect provenance. Hosted cross-platform CI has never run for the Rust branch, and the current default image build was blocked by this host's container CA before a current Trivy/SBOM run. These are release-candidate evidence blockers, so the current recommendation is internal review, not `v1alpha1` RC or stable v1.0.

## Required hardening completed for this release

- Provider-specific options are allowlisted, type-checked, included in plan capability negotiation, and either mapped or rejected. Streaming and programmatic tool calling are rejected rather than ignored.
- Tool input/output schemas are strict; built-in tool kinds have compiler-checked capability/effect/idempotency contracts.
- Provider calls, function-call IDs/results, continuations, effects, checkpoints, audit events, and redacted trace events are durable and publicly inspectable.
- Timeout/transport ambiguity is not automatically retried; confirmed effects survive resume; call IDs are scoped by run; missing credentials fail before run/database creation.
- Non-interactive approvals durably pause, signals cancel safely, JSON errors include available run/trace correlation, and SQLite uses WAL plus a bounded lock wait.
- The packaged CLI, clean-directory quickstart, cron-like empty environment, and non-root/read-only OCI contract have executable acceptance coverage.

## Post-v1 features

These are useful extensions but are not required by the product thesis. They need new deterministic state and compatibility contracts before implementation:

- parallel task execution; `foreach` and matrix expansion; loops; routers; sub-workflows; compensation execution;
- structured agent teams and handoffs;
- model token streaming into CLI/workflow state;
- opt-in MCP reconnection and A2A resubmission with explicit remote reconciliation;
- pack dependency resolution, pack lockfiles, remote fetching, publisher signatures, process-backed pack tools, and a versioned plugin ABI;
- vector memory;
- encrypted application-level persistence and external secret-manager adapters;
- reliable monetary cost enforcement when providers expose sufficient authoritative metadata.

## Explicit non-goals

- Event triggers and calendars: external schedulers trigger `agentctl`.
- MongoDB migration, distributed scheduling, multi-host execution, and distributed storage: the correctness boundary is one local process and SQLite database.
- An in-process OS sandbox or stronger network isolation: allowlists are defense in depth, while containers/VMs, identities, egress policy, and platform sandboxes own isolation.
- Free-form multi-agent conversation control flow: the compiled workflow remains authoritative.

## Current operational limits

- The document API is `v1alpha1`; pin the binary/image version and validate before upgrading.
- Scheduling is sequential (`maxConcurrency: 1`). Separate runs may overlap safely in SQLite, but they can still target the same external resource. Use the external scheduler's overlap controls (`flock`, systemd unit serialization, or Kubernetes `concurrencyPolicy: Forbid`) when effects must not overlap.
- SQLite is local durable state, not a secret vault or distributed lease service. Persist `/state` across container invocations and back it up according to the workflow's recovery needs.
- Filesystem/process/network allowlists are not an OS sandbox. Run untrusted workflows in a restricted container/VM with least-privilege credentials and egress.
- At-most-once model/remote calls can become uncertain in the dispatch/acknowledgement window. Inspect and reconcile externally; use `fork` only when fresh effects are knowingly acceptable.
- Tool-using OpenAI/Azure agents require stored-response continuation. `store: false` is rejected until stateless response-item replay is implemented.
- Anthropic, Google, Azure OpenAI, MCP, and A2A are native and mock-tested in this release, not live-tested. Only the OpenAI GPT-5.6 tool path has live end-to-end evidence.
- The current local OCI runtime evidence is Linux arm64. Linux amd64 is configured in the unpushed Ubuntu CI workflow but has not executed.
- The earlier native arm64 image scan reported no HIGH/CRITICAL findings and produced a CycloneDX SBOM. The current source changes have no fresh completed image-build/scan/SBOM record because this host's container CA blocked dependency retrieval.
