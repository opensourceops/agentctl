# Framework limitation burn-down

This is the authoritative register for the framework-completeness program. It
supersedes roadmap language that classified core durability, recovery,
orchestration, security, or operability work as deferred merely because the
workflow API is young.

Program state values are `open`, `in progress`, and `verified`. They describe
the active work queue and are not final dispositions. Before this program is
complete, every entry must have exactly one final disposition:

- `implemented`
- `redesigned`
- `removed from supported surface`
- `externally blocked`

## Dependency order

1. Persistence foundations: artifact content addressing, schema migration,
   sensitive-field encryption, and durable reconciliation.
2. Recovery contracts: legacy-run analysis, terminal retry, compensation, and
   artifact-independent repair/replay.
3. Deterministic scheduler: parallel commits, conflict detection, bounded
   expansion, conditions, loops, and sub-workflows.
4. Bounded agent composition and event output: structured handoffs and
   streaming.
5. Remote and extension boundaries: MCP, A2A, pack locking, trust, and the
   isolated extension protocol.
6. Optional semantic memory, network/process isolation, and resource budgets.
7. Composite acceptance, container/cross-platform evidence, live OpenAI proof,
   documentation, and adversarial review.

## Register summary

| ID | Category | Program state | Intended final disposition |
| --- | --- | --- | --- |
| ART-001 | Durable artifacts | verified | implemented |
| MIG-001 | Legacy selective repair | verified | implemented |
| EFX-001 | Effect reconciliation | verified | implemented |
| RET-001 | Terminal-run retry | verified | implemented |
| ENC-001 | Sensitive-state encryption | verified | implemented |
| SEC-001 | Secret providers | verified | implemented |
| NET-001 | Network policy | open | implemented |
| ISO-001 | Process isolation | open | redesigned |
| BUD-001 | Resource and cost budgets | open | implemented |
| SCH-001 | Deterministic parallel execution | in progress | implemented |
| DYN-001 | Foreach and matrix | in progress | implemented |
| COND-001 | Conditions and routers | in progress | implemented |
| LOOP-001 | Bounded loops | in progress | implemented |
| SUB-001 | Sub-workflows | open | implemented |
| COMP-001 | Compensation | open | implemented |
| TEAM-001 | Structured teams and handoffs | open | redesigned |
| STR-001 | Streaming | open | implemented |
| MCP-001 | MCP resilience | open | implemented |
| A2A-001 | A2A resilience | open | implemented |
| PACK-001 | Pack resolution and lockfiles | open | implemented |
| TRUST-001 | Pack integrity and signing | open | implemented |
| EXT-001 | Plugin strategy | open | redesigned |
| MEM-001 | Semantic memory | open | implemented |
| PROV-001 | Stateless provider continuation | open | implemented |
| OCI-001 | Container execution | in progress | implemented |
| XPLAT-001 | Cross-platform hosted evidence | open | externally blocked |
| EVENT-001 | Event triggers and calendars | verified | removed from supported surface |
| DIST-001 | Distributed execution and storage | verified | removed from supported surface |
| REG-001 | Hosted public registry | verified | removed from supported surface |
| UI-001 | Hosted UI, chat, and visual orchestration | verified | removed from supported surface |

## Persistence and recovery

### ART-001: Durable content-addressed artifacts

- Current behavior: successful bounded file outputs are atomically ingested
  into an immutable local SHA-256 CAS beside the database. SQLite stores blob
  metadata, per-run/task references, provenance, and ingestion leases.
- User impact: repair, replay, verification, and export continue after the
  source workspace file is deleted.
- Security or durability impact: bytes and metadata form one backup boundary;
  digest verification detects missing/corrupt blobs.
- Product decision: use a local filesystem content-addressed store beside the
  state database. SQLite owns metadata, references, provenance, retention, and
  reachability. Blob bytes never enter ordinary SQLite rows.
- Required implementation: atomic verified ingestion, immutable deduplicated
  blobs, media type and logical-name metadata, run/task references, corruption
  verification, export/materialization, inspection, and reachability GC.
- Migration impact: schema 6 adds CAS metadata/reference/lease tables. Explicit
  legacy analysis/import is tracked separately by MIG-001.
- Tests: duplicate ingestion, partial writes, corrupt/missing/wrong-digest
  blobs, disk failures, concurrent ingestion, traversal/symlink rejection,
  workspace/source deletion, repair, replay, GC, read-only consumption, and
  redaction.
- Examples: durable pipeline and container pipeline.
- Live evidence: bounded OpenAI artifact-producing repair plus offline replay.
- Documentation: artifact store, container mounts, backup, repair, replay, and
  GC.
- Final disposition: implemented and verified by 19 store tests, 38 runtime
  tests, credential-free artifact CLI acceptance, and hardened OCI acceptance.

### MIG-001: Legacy run analysis and upgrade

- Current behavior: `runs analyze` and `runs upgrade` derive only metadata
  proven by retained workflow, plan, effects, outputs, and checksummed
  checkpoints. Unprovable suffixes receive conservative safe repair roots.
- User impact: compatible proven predecessors remain reusable without a full
  fork.
- Security or durability impact: fabricating missing fingerprints or deltas
  would permit unsafe reuse.
- Product decision: implement transactional dry-run analysis and an explicit
  run upgrade. Derive only provable metadata and calculate the earliest safe
  root for everything else.
- Required implementation: `runs upgrade` analysis/apply UX, confidence and
  provenance records, digest derivation, checkpoint-delta reconstruction,
  artifact import, and earliest-safe-boundary output.
- Migration impact: schema 7 records additive transactional upgrades; every
  retained schema fixture remains readable and source execution records remain
  unchanged.
- Tests: schema fixtures 1 through 5, complete/partial/impossible derivation,
  failed-upgrade rollback, dry run, corrupt checkpoints, and boundary choice.
- Examples: legacy analysis followed by selective retry/repair.
- Live evidence: not required; the contract is deterministic.
- Documentation: database migration, compatibility, and operator guidance.
- Final disposition: implemented and verified by retained-schema migration,
  dry-run immutability, artifact-import, failed-upgrade rollback,
  impossible-proof boundary, selective repair, workspace deletion, and offline
  replay tests.

### EFX-001: Complete operator reconciliation

- Current behavior: source effects are immutable. Versioned reconciliation
  records represent `applied`, `not_applied`, and `compensated` conclusions
  with effective runtime projection.
- User impact: an operator can resume from a validated applied result, begin a
  fresh attempt after not-applied/compensated evidence, and safely unblock a
  compatible repair.
- Security or durability impact: operators may resort to unsafe forks or
  out-of-band database edits.
- Product decision: preserve immutable source effects and append versioned
  reconciliation records with one active conclusion.
- Required implementation: list, inspect, and reconcile outcomes `applied`,
  `not_applied`, and `compensated`; identity, timestamp, reason, evidence,
  optional validated result, supersession rules, compensation linkage, audit,
  trace, policy, and non-interactive behavior.
- Migration impact: schema 8 adds reconciliation history and effective-effect
  projection. Existing source effects are never rewritten.
- Tests: every transition, contradictory decisions, supersession, wrong
  schemas, operator policy, repair/resume/retry integration, idempotency keys,
  and transaction rollback.
- Examples: operational workflow with manual applied and compensated outcomes.
- Live evidence: selective repair after an explicitly reconciled mock effect.
- Documentation: effect recovery and honest external-state semantics.
- Final disposition: implemented and verified by transition/supersession,
  contradiction, compensation-link, immutable-source, audit/trace,
  result-schema/tool-contract/hook, policy approval, repair, and both resume
  paths.

### RET-001: Terminal-run retry

- Current behavior: task retry is same-run and bounded; `agentctl retry`
  creates a distinct source-linked run for an unchanged terminal workflow.
- User impact: operational retry of a failed unchanged workflow is obscure.
- Security or durability impact: a fork can repeat successful external effects.
- Product decision: add a distinct source-linked retry plan and run mode. It
  requires the identical workflow definition and reuses compatible success.
- Required implementation: failed-only, selected roots, multiple roots,
  successful-root acknowledgement, plan/JSON/human output, fresh attempts,
  lineage, effect safety, reconciliation, and offline replay.
- Migration impact: new run mode and source/roots metadata are additive.
- Tests: failed-only closure, branches, multiple roots, workflow mismatch,
  explicit successful restart, uncertain effects, source immutability, and
  replay.
- Examples: durable pipeline retry after deterministic downstream failure.
- Live evidence: bounded deterministic provider failure followed by live retry.
- Documentation: retry versus resume, repair, replay, and fork.
- Final disposition: implemented and verified by failed-only and selected-root
  planning, multiple-root and successful-root acknowledgement tests, exact
  workflow identity enforcement, reuse and source-immutability checks,
  uncertain-effect reconciliation coverage, schema-9 lineage persistence,
  offline replay, and packaged CLI acceptance.

### ENC-001: Envelope encryption for sensitive persisted fields

- Current behavior: identified confidential fields can be inventoried and
  transactionally migrated to authenticated envelopes, then fail closed.
- User impact: filesystem disclosure reveals confidential workflow history.
- Security or durability impact: SQLite permissions are not confidentiality at
  rest.
- Product decision: use an established authenticated-encryption crate and a
  versioned envelope for identified sensitive JSON/text fields. Do not claim
  full-database encryption.
- Required implementation: key references, key IDs, authenticated associated
  data, strict no-fallback decryption, rotation, redacted inspection,
  backup/restore guidance, and bounded migration.
- Migration impact: transactional plaintext-to-envelope migration with dry-run
  inventory and rollback on wrong/missing keys.
- Tests: known vectors where provided by the library, wrong key, tampering,
  rotation, mixed versions, migration rollback, and no plaintext remnants in
  protected columns.
- Examples: encrypted state with redacted inspection.
- Live evidence: no provider call required.
- Documentation: protected fields, key lifecycle, and residual metadata.
- Final disposition: implemented and verified by authenticated-context
  roundtrips, missing/wrong-key and tamper failure, dry-run inventory,
  protected-column scans, trigger-enforced no-fallback writes, injected
  rotation rollback, full atomic rotation, schema-10 fixture migration, normal
  read compatibility, checkpoint integrity, and packaged CLI replay.

## Workflow language and runtime

### SCH-001: Deterministic parallel scheduling

- Current behavior: `maxConcurrency` accepts 1 through 64 and defaults to 1.
- User impact: independent model and deterministic tasks cannot overlap.
- Security or durability impact: naive concurrency would make state and effect
  order race-dependent.
- Product decision: execute a stable ready batch concurrently but commit task
  results in compiled order. Tasks declare working-memory write sets; conflicting
  writes fail before dispatch unless an explicit deterministic merge exists.
- Required implementation: concurrency semaphore, isolated task snapshots,
  ordered commit queue, failure/cancellation/approval behavior, effect and trace
  parentage, repair/retry/replay integration, and plan visibility.
- Migration impact: additive DSL/plan fields retain sequential defaults.
  Database migration 11 adds an encrypted-capable per-task execution-memory
  snapshot so crashes and approvals preserve the original batch boundary.
- Tests: real overlap, stable commit order, conflict rejection, cancellation,
  approval, failures, effects, replay, repair, and container execution.
- Examples: parallel deterministic and agent branches.
- Live evidence: two bounded OpenAI branches.
- Documentation: scheduling and state conflict rules.
- Final disposition: implemented. Deterministic verification covers real
  overlap and the concurrency cap, compiled write-conflict rejection, runtime
  declared-key enforcement before effects, atomic rollback injection,
  plan-order audit assertions, disjoint state merge, stop/continue boundary
  behavior, cancellation, multi-approval resume, failed-only retry, selective
  repair, and offline replay. Program state remains in progress until the
  bounded live OpenAI branch scenario and OCI gate execute.

### DYN-001: Bounded foreach and matrix expansion

- Current behavior: static typed foreach lists and matrix axes compile to
  bounded durable child tasks and an ordered aggregate.
- User impact: authors duplicate similar tasks and cannot retry individual
  expanded units.
- Security or durability impact: model-controlled unbounded expansion could
  exhaust resources.
- Product decision: compile static foreach and matrix values only. Runtime or
  model-controlled graph growth is outside the supported surface.
- Required implementation: stable escaped child IDs, item binding, count
  limits, aggregate output, partial-failure rules, child inspection,
  repair/retry/replay, and task budgets.
- Migration impact: plan/checkpoint formats gain expansion records.
- Tests: order, ID collisions, bounds, aggregation, partial failure, individual
  retry/repair, replay, and malformed input.
- Examples: small deterministic and agent matrices.
- Live evidence: two-item OpenAI matrix.
- Documentation: syntax, limits, IDs, and recovery.
- Final disposition: implemented. Static typed lists and Cartesian axes compile
  to stable digest-qualified child IDs and an ordered parent aggregate.
  Deterministic verification covers bounds, malformed bindings, identity,
  output aggregation, partial failure, failed-only child retry, sibling reuse,
  and offline replay. Program state remains in progress until the bounded live
  OpenAI matrix scenario executes.

### COND-001: Typed conditions and routers

- Current behavior: constrained typed conditions and explicit pure router tasks
  persist decisions and skip only enumerated destinations.
- User impact: nontrivial deterministic branching is awkward.
- Security or durability impact: expanding to arbitrary expressions would add
  code execution and ambiguous dependencies.
- Product decision: version the existing constrained expression AST and add a
  typed route selector with enumerated destinations.
- Required implementation: compile-time validation, durable evaluation input
  and decision, skipped-state semantics, changed-input invalidation, plan
  visibility, output option contracts, repair/retry/replay.
- Migration impact: task state and plan versions add condition decisions.
- Tests: types, missing/null, invalid routes, skips, downstream behavior,
  changed decisions, repair, and replay.
- Examples: structured agent output routed to deterministic branches.
- Live evidence: one structured-output routing scenario.
- Documentation: expression grammar and skip semantics.
- Final disposition: implemented. Conditions use the constrained typed
  evaluator and retain expression, context digest, and result. Routers compare
  one exact typed selector against unique JSON cases, record selected value and
  destinations, and durably skip unselected branches. Deterministic
  verification covers strict typing, malformed routes, local vars, plan
  guards, retry, changed-decision repair, skipped-task replay, and zero replay
  effects. Program state remains in progress until the structured OpenAI
  routing scenario executes.

### LOOP-001: Bounded loops

- Current behavior: a task-level loop compiles into a bounded sequential chain
  of durable iteration tasks and one pure aggregate.
- User impact: bounded refine/verify workflows require duplicated tasks.
- Security or durability impact: an unbounded model-owned loop violates the
  runtime's bounded-execution thesis.
- Product decision: implement a durable loop construct with typed condition,
  explicit maximum iterations, and iteration-local output/state boundaries.
- Required implementation: stable iteration IDs, durable iteration state,
  outputs, cancellation, effect identities, repair/retry at boundaries, replay,
  and loop/resource budgets.
- Migration impact: the DSL and compiled plan gain additive loop records.
  Iterations use existing task, checkpoint, effect, and attempt storage, so no
  SQLite migration is required.
- Tests: zero/one/max iterations, bound exceeded, cancellation, uncertain
  effect, repair/retry, and replay.
- Examples: bounded operational verification loop.
- Live evidence: a two-iteration maximum agent scenario.
- Documentation: loop safety and recovery.
- Final disposition: implemented. Iteration IDs and bindings are stable,
  `maxIterations` is required and capped at 64, typed guards run before each
  iteration, false guards durably skip the remaining chain, and a still-true
  final guard fails closed. Deterministic verification covers zero, one, and
  maximum iterations, bound exhaustion, cancellation with an uncertain
  in-flight provider effect, per-boundary retry and repair, offline replay, and
  zero replay effects. Packaged CLI scenario 36 verifies plan, run, inspect,
  and replay. Program state remains in progress until the bounded live agent
  scenario executes.

### SUB-001: Reusable sub-workflows

- Current behavior: packs can contribute actions/agents/tools but not workflows.
- User impact: reusable graph composition requires copying tasks.
- Security or durability impact: implicit policy/provider inheritance could
  broaden authority.
- Product decision: compile versioned sub-workflows into namespaced tasks with
  explicit typed inputs/outputs and monotonic policy inheritance.
- Required implementation: pack/local definitions, namespace escaping,
  recursion/cycle checks, state isolation, provider mapping, artifact ownership,
  lineage, errors, inspection, repair/retry/replay.
- Migration impact: pack/lock, workflow schema, and plan format.
- Tests: nesting, collisions, cycles, policy narrowing, output contracts,
  failures, artifacts, repair/retry/replay.
- Examples: operational workflow calling a reusable sub-workflow.
- Live evidence: sub-workflow containing one OpenAI task.
- Documentation: authoring, versioning, and policy inheritance.
- Final disposition: pending implementation evidence.

### COMP-001: Explicit compensation

- Current behavior: tool contracts carry compensation metadata but runtime does
  not execute it.
- User impact: operators cannot durably coordinate best-effort reversal.
- Security or durability impact: documentation-shaped metadata can be mistaken
  for transactional rollback.
- Product decision: compensation is an explicit new run phase in reverse
  dependency order, never a transactional rollback claim.
- Required implementation: declaration validation, manual trigger, opt-in
  automatic trigger, approval, idempotency, partial failure, linkage to effects
  and reconciliation, audit, trace, retry/repair behavior.
- Migration impact: effect links, run phase, and checkpoints.
- Tests: order, approval, idempotency, partial failure, contradictory
  reconciliation, cancellation, and replay.
- Examples: operational workflow compensation.
- Live evidence: deterministic tool compensation only.
- Documentation: guarantees and non-guarantees.
- Final disposition: pending implementation evidence.

### TEAM-001: Structured teams and handoffs

- Current behavior: free-form agent handoffs are intentionally absent.
- User impact: users cannot name bounded roles and inspect typed handoffs.
- Security or durability impact: hidden agent conversations would bypass the
  compiled graph and policy.
- Product decision: redesign "teams" as syntactic composition over explicit
  tasks/sub-workflows. No autonomous hidden conversation scheduler is added.
- Required implementation: role declarations, bounded turn count, typed handoff
  payload, explicit route conditions, per-role tools/policy, durable handoff
  records, and ordinary repair/retry/replay.
- Migration impact: new syntax compiles to existing versioned task constructs.
- Tests: policy separation, handoff schemas, turn limits, routes, failures,
  repair/retry/replay, and inspection.
- Examples: three-role structured verification workflow.
- Live evidence: two-role OpenAI handoff plus deterministic verifier.
- Documentation: explain the compiled replacement and reject free-form teams.
- Final disposition: pending redesign evidence.

### STR-001: End-to-end streaming

- Current behavior: provider transports parse some SSE but workflow output is a
  final document only.
- User impact: long agent calls have no bounded progress stream.
- Security or durability impact: unbounded deltas or mixed JSON output can leak
  secrets and corrupt automation.
- Product decision: add durable bounded stream events and explicit human or
  JSONL progress modes while retaining one final JSON document mode.
- Required implementation: provider fragments, sequence numbers, persisted
  bounded/redacted records, backpressure, cancellation, reconnect semantics,
  final result validation, and recorded stream replay.
- Migration impact: stream-event table and CLI output contract.
- Tests: fragmented events, backpressure, truncation, redaction, cancellation,
  replay, reconnect, and final JSON isolation.
- Examples: streaming agent workflow.
- Live evidence: one packaged OpenAI streaming run.
- Documentation: stdout contracts and replay.
- Final disposition: pending implementation evidence.

## Remote protocols, packs, and memory

### MCP-001: Safe MCP reconnect

- Current behavior: session expiry fails explicitly and requires external
  recovery.
- User impact: safe observations cannot recover from server restart, and manual
  recovery lacks protocol-specific evidence.
- Security or durability impact: automatic retry of an uncertain mutation can
  duplicate work.
- Product decision: bounded reconnect is allowed before dispatch and for proven
  observations/idempotent calls. Uncertain mutating calls require EFX-001.
- Required implementation: reinitialize, tool-list/schema refresh, auth refresh,
  server restart handling, reconnect budget, call identity, streaming, and
  repair/retry/replay integration.
- Migration impact: protocol session and call records gain generation/status.
- Tests: restart at each lifecycle boundary, schema change, auth refresh,
  cancellation, timeout, and no duplicate mutation.
- Examples: operational mock MCP workflow.
- Live evidence: deterministic local server only.
- Documentation: safe reconnect matrix.
- Final disposition: pending implementation evidence.

### A2A-001: Safe remote-task continuation

- Current behavior: task polling is bounded, but task identity is not exposed as
  a complete resumable reconciliation workflow.
- User impact: a lost response can strand externally running work.
- Security or durability impact: blind `SendMessage` resubmission duplicates a
  remote task.
- Product decision: persist external task IDs before polling and resume polling
  or streaming; never resubmit an ambiguous task automatically.
- Required implementation: card refresh, interface compatibility, task ID and
  stream cursor persistence, auth refresh, artifact retrieval into ART-001,
  cancellation, bounded retry, and EFX-001 linkage.
- Migration impact: protocol task/session records.
- Tests: ambiguous submission, polling/stream resume, card/interface change,
  auth refresh, cancellation, artifacts, repair/retry/replay.
- Examples: resilient mock A2A workflow.
- Live evidence: deterministic local peer only.
- Documentation: continuation and reconciliation.
- Final disposition: pending implementation evidence.

### PACK-001: Deterministic pack resolution and lockfile

- Current behavior: only contained local manifests with direct integrity work.
- User impact: reusable content has no dependencies, Git/archive source, locked
  graph, or offline resolution.
- Security or durability impact: ad hoc fetching weakens reproducibility.
- Product decision: support local path, pinned Git commit, and immutable HTTPS
  archive sources with semantic constraints and a checked-in lockfile. No hosted
  registry is required.
- Required implementation: resolver, cycles/conflicts, canonical graph,
  integrity, offline/locked modes, cache, and update command.
- Migration impact: pack reference and manifest versions plus lockfile v1.
- Tests: constraints, conflicts, cycles, tamper, offline, locked drift, Git
  pinning, archive limits, and path escape.
- Examples: transitive local packs and pinned archive fixture.
- Live evidence: not required.
- Documentation: source/trust/lock workflows.
- Final disposition: pending implementation evidence.

### TRUST-001: Pack authenticity and trust policy

- Current behavior: SHA-256 proves sameness but not publisher identity.
- User impact: users must establish provenance manually.
- Security or durability impact: a valid digest from an untrusted source can
  still execute dangerous content.
- Product decision: integrate optional Sigstore-compatible bundle verification
  and explicit unsigned policy. Do not invent cryptography.
- Required implementation: identity/issuer allowlists, offline bundle
  verification where possible, locked digest binding, unsigned deny/warn/allow,
  and process-tool trust gating.
- Migration impact: lockfile trust metadata and policy fields.
- Tests: trusted/untrusted/expired/malformed bundles, unsigned policy, digest
  mismatch, and no process execution before trust.
- Examples: signed-fixture verification and explicit unsigned local pack.
- Live evidence: deterministic verification fixture.
- Documentation: trust model and keyless-signing caveats.
- Final disposition: pending implementation evidence.

### EXT-001: Isolated extension model

- Current behavior: no executable plugin ABI; MCP and built-in process actions
  are separate surfaces.
- User impact: "plugin ABI" appears as an unresolved roadmap item.
- Security or durability impact: an in-process native ABI would undermine Rust
  safety and process isolation.
- Product decision: remove native ABI from the supported surface. The supported
  extension contracts are reviewed packs plus MCP or a versioned bounded process
  protocol.
- Required implementation: process-protocol handshake, version negotiation,
  declared schemas/capabilities, direct argv, timeout/output/cancellation,
  policy, and effect identity. MCP remains the network extension option.
- Migration impact: pack action kinds and compatibility guide.
- Tests: version/schema mismatch, output overflow, timeout, cancellation,
  policy, secret environment, and crash.
- Examples: local process-protocol extension.
- Live evidence: not required.
- Documentation: definitive plugin strategy and rejection of native libraries.
- Final disposition: pending redesign evidence.

### MEM-001: Optional semantic retrieval

- Current behavior: long-term memory supports namespace/key lookup only.
- User impact: workflows cannot retrieve relevant prior entries by text or
  vectors.
- Security or durability impact: implicit model memory could bypass retention
  and replay boundaries.
- Product decision: add typed entries with deterministic text search, optional
  local vector/hybrid search, explicit promotion, namespaces, filters, and
  retention. Retrieval remains an effect and replay uses recorded results.
- Required implementation: provider-neutral embedding interface, deterministic
  fake embedder, local index, optional OpenAI adapter, external adapter trait,
  filters, ranking, and explicit promotion.
- Migration impact: memory schema and index version.
- Tests: deterministic ranking, filters, namespaces, retention, repair/replay,
  index rebuild, corrupt dimensions, and fake embeddings.
- Examples: hybrid retrieval and promotion.
- Live evidence: one bounded embedding scenario only if publicly exposed.
- Documentation: memory versus provider cache and working state.
- Final disposition: pending implementation evidence.

### PROV-001: Stateless tool continuation

- Current behavior: OpenAI/Azure tool agents reject `store: false`.
- User impact: privacy-sensitive users cannot opt out of stored provider
  responses for tool loops.
- Security or durability impact: pretending support would lose reasoning and
  function-call items needed for correct continuation.
- Product decision: persist provider-neutral opaque returned items needed for
  stateless continuation and replay them on the next request.
- Required implementation: versioned continuation items, provider mapping,
  size/redaction bounds, encryption under ENC-001, and capability negotiation.
- Migration impact: provider continuation format.
- Tests: multiple tools, reasoning items, cancellation, resume, repair session
  freshness, encrypted persistence, and malformed items.
- Examples: stateless OpenAI tool workflow.
- Live evidence: one packaged `store: false` OpenAI tool run.
- Documentation: stateful versus stateless continuation.
- Final disposition: pending implementation evidence.

## Security and operations

### SEC-001: Stable secret-reference providers

- Current behavior: environment, bounded mounted-file, and policy-gated direct
  process references work across provider credentials, provider/protocol
  headers, and action environments.
- User impact: container-native secret files and reviewed credential helpers
  work without wrapper scripts or secret-valued workflow inputs.
- Security or durability impact: resolved values stay in zeroizing memory,
  while effect records retain only source descriptions and value digests.
- Product decision: version secret references for environment, bounded mounted
  file, and optional direct process provider. Resolved values never persist.
- Required implementation: canonical file-root containment, dedicated process
  allowlists, direct argv, cleared environments, process groups,
  timeout/output/cancellation bounds, redaction registration, and lifecycle
  zeroization.
- Migration impact: existing `{env: NAME}` remains valid.
- Tests: missing/oversized/symlink files, denied commands, timeout, redaction,
  and database/trace absence.
- Examples: environment and read-only mounted-file container contracts in the
  secret-reference guide and container documentation.
- Live evidence: OpenAI credential remains environment-only for task evidence.
- Documentation: secret reference types and threat model.
- Final disposition: implemented and verified by DSL compatibility and policy
  tests; missing, oversized, and symlink-escape file tests; denied, timed-out,
  output-limited, and cancelled process tests; provider adapter redaction; raw
  SQLite absence checks; and packaged CLI acceptance scenario 32.

### NET-001: Network destination enforcement

- Current behavior: exact/wildcard host grants and disabled redirects exist.
- User impact: users cannot constrain ports, schemes, private networks, proxies,
  Unix sockets, custom CAs, or response size consistently.
- Security or durability impact: DNS rebinding, proxy routing, and local-service
  access remain residual SSRF paths.
- Product decision: resolve and validate each destination against scheme, host,
  port, IP class, proxy, redirect, and TLS/CA policy at the adapter boundary.
- Required implementation: resolved-IP checks, private-range controls,
  rebinding defense, explicit proxy and Unix-socket denial, response limits,
  shared timeouts, and protected custom-CA references.
- Migration impact: policy schema defaults preserve current public HTTPS use.
- Tests: DNS changes, private/link-local/loopback IPs, ports, schemes,
  redirects, proxies, CA success/failure, oversized responses, and IPv6.
- Examples: constrained MCP/provider policies.
- Live evidence: public OpenAI route under explicit HTTPS/443 policy.
- Documentation: network model and external egress boundary.
- Final disposition: pending implementation evidence.

### ISO-001: Honest process isolation

- Current behavior: direct argv, cleared environment, output/time bounds, and
  process-group termination exist, but policy is not an OS sandbox.
- User impact: users may overestimate allowlists.
- Security or durability impact: an allowed executable has the host identity's
  full authority.
- Product decision: require an explicit isolation mode. `process` is the honest
  host mode; `container` is the portable strong boundary. Optional platform
  backends can be added when detected, but no weak emulation is claimed.
- Required implementation: DSL/plan visibility, fail-closed requested backend,
  resource limits where supported, container invocation contract, and explicit
  unsupported diagnostics on macOS/Windows/Linux backends.
- Migration impact: existing actions default to documented host-process mode.
- Tests: environment, working directory, process tree, resource bounds,
  unavailable backend, and container isolation.
- Examples: host and container-isolated process action.
- Live evidence: container acceptance only.
- Documentation: policy versus isolation.
- Final disposition: pending redesign evidence.

### BUD-001: Enforceable resource and cost budgets

- Current behavior: per-agent turns/tokens and per-process output/time have
  partial limits.
- User impact: no run-wide provider, tool, token, artifact, task, wall-time, or
  cost ceiling.
- Security or durability impact: a bounded individual task can still compose
  into an expensive run.
- Product decision: compile task/run budgets, reserve known units before
  dispatch, reconcile actual usage after each effect, and fail safely when the
  next known operation would exceed a hard bound.
- Required implementation: requests, turns, tool calls, token classes, wall
  time, process output, artifact bytes, task/expansion/loop counts, and optional
  versioned/custom pricing.
- Migration impact: DSL, plan, checkpoint, audit, and usage records.
- Tests: each bound, parallel reservation, unknown pricing, custom pricing,
  retries, repair/replay, and off-by-one behavior.
- Examples: budget termination and usage inspection.
- Live evidence: one low request/token ceiling OpenAI scenario.
- Documentation: enforceable versus estimated limits.
- Final disposition: pending implementation evidence.

### OCI-001: Complete container execution

- Current behavior: OCI acceptance exists, but baseline execution on this host
  failed because `/artifacts/report.txt` was rejected by workspace path policy.
- User impact: the documented separate artifact mount is not usable through the
  real current acceptance path.
- Security or durability impact: weakening workspace containment would be an
  unsafe fix.
- Product decision: make the artifact store part of the writable `/state`
  contract and materialize exports only through an explicitly authorized
  artifact-export root. Detect usable Docker/Podman-compatible engines
  truthfully.
- Required implementation: runtime detection, persistent Podman handling,
  non-root/read-only runs, mounted config/workspace/state/export roots, ART-001,
  retry/repair/reconciliation/replay, signals, limits, CA extension, SBOM,
  vulnerability/history/secret inspection, and multi-architecture builds.
- Migration impact: container mount documentation and default state paths.
- Tests: deterministic contract tests plus native/emulated execution labels.
- Examples: composite container workflow.
- Live evidence: OpenAI container scenario only when runtime remains usable.
- Documentation: runtime troubleshooting and mount migration.
- Final disposition: pending implementation evidence.

### XPLAT-001: Hosted platform evidence

- Current behavior: GitHub workflows are locally linted but undispatched.
- User impact: Linux x64, hosted macOS, and Windows claims lack exact-commit
  evidence.
- Security or durability impact: platform-specific path, process, packaging, and
  migration bugs may remain.
- Product decision: configure complete least-privilege hosted matrices but do
  not claim execution during this no-push task.
- Required implementation: build/test/acceptance/package/examples/completeness
  jobs for macOS ARM64, Linux ARM64/x64, Windows x64, and container/security
  jobs with artifacts and digests.
- Migration impact: none.
- Tests: local workflow lint, action pin scan, and matrix completeness check.
- Examples: all public examples inventoried by jobs.
- Live evidence: not run because hosted dispatch is explicitly prohibited.
- Documentation: exact blocker and continuation.
- Final disposition: pending external-blocker evidence.

## Removed unsupported surface

### EVENT-001: Event triggers and calendars

- Current behavior: external schedulers invoke the CLI.
- Product decision: remove event/calendar scheduling from the limitations list.
  It is outside the deterministic single-run runtime thesis.
- Required implementation: reject any event-trigger DSL fields and keep cron,
  systemd, CI, and Kubernetes invocation guides.
- Compatibility impact: no current supported syntax changes.
- Tests: strict unknown-field rejection.
- Final disposition: `removed from supported surface`.

### DIST-001: Distributed execution and storage

- Current behavior: one local process and SQLite are the correctness boundary.
- Product decision: distributed scheduling, leases, multi-host execution, and
  distributed storage are explicit non-goals, not incomplete core behavior.
- Required implementation: remove roadmap ambiguity and make local ownership
  explicit.
- Compatibility impact: none.
- Tests: documentation/product-boundary verification.
- Final disposition: `removed from supported surface`.

### REG-001: Hosted public registry

- Current behavior: no hosted pack service.
- Product decision: a public registry is unnecessary. PACK-001 supports local,
  Git, and immutable archive sources without a hosted control plane.
- Required implementation: remove public-registry roadmap claims.
- Compatibility impact: none.
- Tests: source resolver coverage.
- Final disposition: `removed from supported surface`.

### UI-001: Hosted UI, chat, and visual orchestration

- Current behavior: the CLI and embeddable Rust runtime are authoritative.
- Product decision: hosted SaaS, IDE, visual editor, chat application, and
  free-form conversation orchestration are explicit non-goals.
- Required implementation: reject hidden model-owned control flow and document
  TEAM-001 as compiled workflow syntax.
- Compatibility impact: none.
- Tests: compiler rejects unsupported control-flow fields.
- Final disposition: `removed from supported surface`.

## Baseline evidence

Recorded on 2026-07-23 before framework-completeness implementation:

- `cargo xtask verify`: passed all 12 stages.
- `cargo xtask acceptance`: passed 28 scenarios.
- `cargo xtask examples-verify`: passed.
- `cargo xtask docs-verify`: passed.
- `cargo xtask package`: passed.
- `cargo xtask secret-scan`: passed.
- `env -u OPENAI_API_KEY cargo xtask acceptance-container`: the Podman VM
  required a persistent terminal to keep forwarding alive; after reaching the
  OCI binary, acceptance failed with exit 3 because
  `/artifacts/report.txt` escaped the authorized workspace root. No credential
  was supplied and no OpenAI call occurred.
