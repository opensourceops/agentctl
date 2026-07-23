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
- Shell execution and acceptance/container helpers use bounded concurrent capture. Output overflow terminates/reaps the child with a structured secret-safe error; timeouts and cancellation retain durable uncertain-effect semantics.
- Hosted workflows use least privilege, full-SHA action pins with version comments, complete-history/tree Gitleaks, deterministic fake-secret detection, dependency/image scans, and required production/image CycloneDX artifacts with digests.

## Post-v1 features

These are useful extensions but are not required by the product thesis. They need new deterministic state and compatibility contracts before implementation:

- parallel task execution; `foreach` and matrix expansion; loops; routers; sub-workflows; compensation execution;
- structured agent teams and handoffs;
- model token streaming into CLI/workflow state;
- opt-in MCP reconnection and A2A resubmission with explicit remote reconciliation;
- pack dependency resolution, pack lockfiles, remote fetching, publisher signatures, and a versioned plugin ABI;
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
- Selective repair requires task metadata version 1. Successful tasks from databases created before schema 5 remain inspectable but must execute from an earlier repair root or a full fork.
- Automatic artifact manifests cover bounded files reported by successful workspace-mutation results. Artifact bytes are not copied into SQLite or a content-addressed store; retain the configured workspace. Missing, moved, size-mismatched, or digest-mismatched bytes block repair before run creation and report the expected artifact identity.
- A confirmed non-idempotent mutation in a repair closure remains blocked. The only built-in reconciliation outcome is an operator-confirmed `not-applied` result for a started or uncertain effect; compensation and provider-specific deduplication workflows are not implemented.
- Retry remains a bounded same-run task policy. There is no separate command that creates a new terminal-source retry run for an unchanged workflow; use repair with an unchanged target definition and explicit roots when its compatibility checks fit.
- Tool-using OpenAI/Azure agents require stored-response continuation. `store: false` is rejected until stateless response-item replay is implemented.
- Anthropic, Google, Azure OpenAI, MCP, and A2A are native and mock-tested in this release, not live-tested. Only the OpenAI GPT-5.6 tool path has live end-to-end evidence.
- The current local OCI runtime, vulnerability-scan, and SBOM evidence is Linux arm64. Linux x64 is configured in the unpushed Ubuntu workflow but has not executed.
- GitHub runner availability, organization action policy, branch protection, and required-check configuration are repository-owner operations and cannot be proven by repository-local lint.
