# Supported boundaries and operational limits

This classification is part of the product contract. Core runtime limitations
are closed in the
[framework limitation burn-down](execution/LIMITATION_BURNDOWN.md).
Capabilities outside the product thesis are explicit non-goals, and
environment-specific evidence is labeled separately from implementation.

## Release blockers

No known P0/P1 implementation defect remains for the stated local, scheduled,
and OCI journeys. The local container build has a secure optional CA secret
path, and exact-head pull-request gates execute Linux x64, hosted macOS arm64,
Windows x64, container, security, package, and SBOM validation without
provider credentials. Exact run and artifact digests belong to the independent
candidate report. Version 0.3 publishes workflow API `agentctl.dev/v1`; the CLI
and crates remain pre-1.0 and do not imply long-term support.

## Required hardening completed for this release

- Provider-specific options are allowlisted, type-checked, included in plan capability negotiation, and either mapped or rejected. Streaming is explicit and capability-checked; programmatic tool calling is rejected rather than ignored.
- Tool input/output schemas are strict; built-in tool kinds have compiler-checked capability/effect/idempotency contracts.
- Provider calls, function-call IDs/results, continuations, effects, checkpoints, audit events, and redacted trace events are durable and publicly inspectable.
- Timeout/transport ambiguity is not automatically retried; confirmed effects survive resume; call IDs are scoped by run; missing credentials fail before run/database creation.
- Non-interactive approvals durably pause, signals cancel safely, JSON errors include available run/trace correlation, and SQLite uses WAL plus a bounded lock wait.
- The packaged CLI, clean-directory quickstart, cron-like empty environment, and non-root/read-only OCI contract have executable acceptance coverage.
- Successful bounded file outputs are atomically ingested into a local immutable content-addressed store with durable references, verification/export commands, lease-safe reachability GC, interrupted-GC recovery, and local/OCI acceptance coverage.
- Identified confidential JSON and text fields can be transactionally migrated to versioned AES-256-GCM envelopes, rotated through environment key references, inventoried without content disclosure, and fail closed on missing/wrong keys, tampering, plaintext writes, or stale-key writes.
- Shell execution and acceptance/container helpers use bounded concurrent capture. Output overflow terminates/reaps the child with a structured secret-safe error; timeouts and cancellation retain durable uncertain-effect semantics.
- Process actions expose plan-visible `process` or `container` isolation.
  Host mode is explicitly not a sandbox. Container mode requires a local
  digest-pinned Docker/Podman image and runs networkless, read-only, non-root,
  capability-dropped, and memory/CPU/PID/output/time bounded without host
  fallback.
- Optional run-wide budgets enforce provider requests, turns, tool calls,
  input/output/total tokens, durable wall time, captured process output,
  ingested artifact bytes, compiled task/expansion/loop counts, and cost when
  explicit versioned pricing is configured.
- Hosted workflows use least privilege, full-SHA action pins with version comments, complete-history/tree Gitleaks, deterministic fake-secret detection, dependency/image scans, and required production/image CycloneDX artifacts with digests.

## Optional integration boundaries

These are extension points, not incomplete core runtime behavior:

- external secret-manager adapters beyond environment, mounted-file, and policy-gated process references;
- provider-maintained pricing discovery. Monetary limits require authoritative
  response cost or operator-supplied versioned custom pricing.

## Explicit non-goals

- Event triggers and calendars: external schedulers trigger `agentctl`.
- MongoDB migration, distributed scheduling, multi-host execution, and distributed storage: the correctness boundary is one local process and SQLite database.
- Native Linux namespace/bubblewrap, macOS sandbox-profile, and Windows
  restricted-token/job-object isolation backends. Explicit action container
  mode and externally managed containers/VMs provide the supported isolation
  boundaries.
- Free-form multi-agent conversation control flow: the compiled workflow remains authoritative.

## Current operational limits

- The document API is `agentctl.dev/v1`. Pin the binary or image version and
  validate before upgrading because runtime, provider, and storage contracts
  have independent versions.
- Parallel scheduling is local to one run and process, bounded at 64 tasks, and defaults to sequential execution. Working-memory conflicts fail compilation, but tasks that target the same external resource still require explicit `needs` ordering or that system's concurrency controls. Separate runs also require external overlap controls when effects must not overlap.
- Run budgets are optional. Provider input tokens and future token classes are
  estimated conservatively before dispatch; actual provider usage is
  reconciled after the response. Custom pricing is operator-maintained and is
  not automatically refreshed from public price pages.
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
- Streaming is available for fake, OpenAI, and Azure OpenAI agents only.
  Progress records are capped at 256 events per task attempt and 4 KiB per
  payload. The OpenAI SSE transport is capped at 8 MiB. Transport loss is not
  automatically reconnected or resubmitted, and final task output still
  requires a terminal validated provider response.
- MCP reconnects at most once only for `pure`, `idempotent`, or `keyed`
  actions and only while the selected tool schema remains identical. Calls
  with `unknown` or `at_most_once` idempotency remain uncertain after a lost
  response and require reconciliation.
- A2A continuation requires a persisted remote task ID. An ambiguous
  `SendMessage` without a returned task ID is never resubmitted automatically.
  `effects continue-remote` can resume polling or streaming of a known task,
  retrieve bounded same-origin artifacts, and make the recovered boundary
  reusable by retry.
- Pack resolution has no hosted registry or version discovery. Every root and
  dependency names one local, pinned Git, or immutable archive source. The
  checked-in lock is per workflow directory. Sigstore verification uses the
  trust root embedded in the installed agentctl version; rotate agentctl when
  public-good trust material changes.
- `extension.process` is a reviewed process contract, not a native ABI or OS
  sandbox. Handshake is a non-mutating protocol promise; invocation failures
  after dispatch remain uncertain. Set `isolation: container` or use a stronger
  externally managed platform boundary for hostile executables.
- SQLite memory search scans at most 10,000 active entries and returns at most
  100 results. Its `local_hash` vectors are deterministic lexical features, not
  neural embeddings. Use the optional OpenAI embedding adapter or implement the
  public embedding and memory adapter traits when semantic quality or a
  specialized external index is required. Promotion into run working memory is
  always explicit.
- SQLite is local durable state, not a secret vault or distributed lease service. Persist `/state` across container invocations and back it up according to the workflow's recovery needs.
- State encryption is explicit and selected-field only. Before it is enabled, the database is plaintext. It does not encrypt artifact bytes or operational metadata, and it cannot retroactively protect old backups or snapshots. Preserve the current referenced key with encrypted backups.
- Network preflight validates the HTTP(S) scheme, host, effective port, and
  every resolved IPv4/IPv6 address before run creation, then pins accepted
  direct DNS answers. Private addresses and environment proxies are denied by
  default; redirects and Unix sockets are disabled. Explicit `allowProxy`
  delegates routing and destination resolution to that trusted proxy, so use
  external egress isolation for hostile workflows.
- Filesystem/process/network allowlists and `isolation: process` are not an OS
  sandbox. Individual `isolation: container` actions are networkless and mount
  only the authorized working directory read-only, but still trust the engine,
  image, and explicitly passed secrets. Run untrusted whole workflows in a
  dedicated container/VM identity with external resource and egress controls.
- At-most-once model/remote calls can become uncertain in the dispatch/acknowledgement window. Inspect and reconcile externally; use `fork` only when fresh effects are knowingly acceptable.
- Successful tasks from databases created before schema 5 require explicit `runs analyze`/`runs upgrade`. Only provable metadata is imported; unprovable boundaries are returned as conservative safe repair roots.
- Automatic artifact ingestion covers regular files up to 16 MiB reported by successful built-in workspace-mutation results. Larger outputs and artifacts produced only by opaque external effects require an explicit bounded import/export integration. The local CAS must be backed up with SQLite; missing or corrupt blob bytes block repair before run creation and report the expected artifact identity.
- An applied non-idempotent mutation in a repair closure remains blocked from duplicate execution unless a confirmed compensation is linked. Reconciliation supports immutable `applied`, `not_applied`, and `compensated` records, validated results, policy authorization, and operation-specific verification hooks; it does not provide exactly-once delivery.
- Terminal retry requires an identical workflow digest and a terminal source. It creates a new source-linked run, reuses only proven compatible successful boundaries, and freshly executes the selected closure. Use repair for a changed definition, resume for a non-terminal run, replay for effect-free reconstruction, and fork for knowingly broad fresh execution.
- Anthropic, Google, Azure OpenAI, MCP, and A2A are native and mock-tested in
  this release, not live-tested. OpenAI GPT-5.6 has bounded live evidence for
  basic and tool agents, parallel branches, matrix tasks, structured routing,
  loops, sub-workflows, typed handoffs, retry, selective repair, artifact CAS
  reuse, keyless replay, streaming, resource-budget termination, and native
  Linux arm64 container execution.
- Local OCI runtime evidence is Linux arm64. The hosted container runtime,
  vulnerability scan, and image SBOM run on Linux x64 and are labeled
  separately from that local evidence.
- GitHub runner availability, organization action policy, branch protection, and required-check configuration are repository-owner operations and cannot be proven by repository-local lint.
