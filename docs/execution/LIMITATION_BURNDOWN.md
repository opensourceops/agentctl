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
| NET-001 | Network policy | verified | implemented |
| ISO-001 | Process isolation | in progress | implemented |
| BUD-001 | Resource and cost budgets | verified | implemented |
| SCH-001 | Deterministic parallel execution | in progress | implemented |
| DYN-001 | Foreach and matrix | in progress | implemented |
| COND-001 | Conditions and routers | in progress | implemented |
| LOOP-001 | Bounded loops | in progress | implemented |
| SUB-001 | Sub-workflows | in progress | implemented |
| COMP-001 | Compensation | verified | implemented |
| TEAM-001 | Structured teams and handoffs | in progress | redesigned |
| STR-001 | Streaming | in progress | implemented |
| MCP-001 | MCP resilience | verified | implemented |
| A2A-001 | A2A resilience | verified | implemented |
| PACK-001 | Pack resolution and lockfiles | verified | implemented |
| TRUST-001 | Pack integrity and signing | verified | implemented |
| EXT-001 | Plugin strategy | verified | redesigned |
| MEM-001 | Semantic memory | verified | implemented |
| PROV-001 | Stateless provider continuation | verified | implemented |
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

- Current behavior: inline and integrity-pinned pack definitions compile into a
  typed input boundary, namespaced ordinary child tasks, and a typed output
  aggregate.
- User impact: reusable graph composition requires copying tasks.
- Security or durability impact: implicit policy/provider inheritance could
  broaden authority.
- Product decision: compile versioned sub-workflows into namespaced tasks with
  explicit typed inputs/outputs and monotonic policy inheritance.
- Required implementation: pack/local definitions, namespace escaping,
  recursion/cycle checks, state isolation, provider mapping, artifact ownership,
  lineage, errors, inspection, repair/retry/replay.
- Migration impact: workflow and pack schemas gain additive reusable workflow
  definitions. Compiled plans gain pure boundary variants; existing task,
  effect, artifact, checkpoint, and attempt storage is reused.
- Tests: nesting, collisions, cycles, policy narrowing, output contracts,
  failures, artifacts, repair/retry/replay.
- Examples: operational workflow calling a reusable sub-workflow.
- Live evidence: sub-workflow containing one OpenAI task.
- Documentation: authoring, versioning, and policy inheritance.
- Final disposition: implemented. Definitions carry a semantic version and
  JSON Schema input/output interfaces. Nested calls flatten recursively and
  cycles fail compilation. The caller's policy and providers remain
  authoritative, deterministic memory keys are invocation-prefixed, and
  namespaced children retain artifact/effect ownership and ordinary
  retry/repair/replay lineage. Focused compiler and runtime verification covers
  stable expansion, typed rejection, state isolation, selected-boundary retry
  and repair, and zero-effect replay. Packaged CLI scenario 37 and the
  integrity-pinned pack example pass. Program state remains in progress until
  the bounded live sub-workflow scenario executes.

### COMP-001: Explicit compensation

- Current behavior: compensable tasks declare a named effectful inverse action.
  `agentctl compensate` plans and executes eligible source effects through a
  separate source-linked run.
- User impact: operators cannot durably coordinate best-effort reversal.
- Security or durability impact: documentation-shaped metadata can be mistaken
  for transactional rollback.
- Product decision: compensation is an explicit sequential run in reverse
  compiled graph order, never a transactional rollback claim.
- Required implementation: declaration validation, manual trigger, opt-in
  automatic trigger, approval, idempotency, partial failure, linkage to effects
  and reconciliation, audit, trace, retry/repair behavior.
- Migration impact: the additive `compensation` run mode reuses existing source
  lineage, task, effect, approval, checkpoint, audit, trace, and reconciliation
  storage. No database schema migration is required.
- Tests: order, approval, idempotency, partial failure, source and inverse
  uncertainty reconciliation, terminal inverse-run blocking, cancellation,
  retry/repair invalidation, and replay.
- Examples: operational workflow compensation.
- Live evidence: deterministic tool compensation only.
- Documentation: guarantees and non-guarantees.
- Final disposition: implemented. Source effects remain immutable; eligible
  confirmed mutations execute declared action-based compensation in reverse
  order. Successful inverse effects append linked `compensated`
  reconciliations. Manual and explicitly automatic triggers, approval,
  uncertainty blocking, bounded retries, partial continuation, repeat planning,
  selected tasks, repair/retry invalidation, and effect-free replay use the
  ordinary durable runtime. Focused compiler/runtime tests, packaged CLI
  scenario 38, the 12-stage verification gate, examples, docs, packaging, and
  secret scanning pass.

### TEAM-001: Structured teams and handoffs

- Current behavior: named bounded agent tasks exchange typed payloads through
  explicit deterministic handoff tasks and reusable sub-workflows.
- User impact: roles, payloads, routing, tool visibility, and recovery
  boundaries remain visible in the ordinary graph.
- Security or durability impact: hidden agent conversations would bypass the
  compiled graph and policy.
- Product decision: redesign teams as explicit agent tasks, typed deterministic
  handoff tasks, routers, and reusable sub-workflows. No autonomous hidden
  conversation scheduler or `team:` DSL is added.
- Required implementation: bounded roles, typed handoff payloads, explicit
  route conditions, per-role provider/tool visibility, durable handoff records,
  and ordinary cancellation, repair, retry, replay, audit, and tracing.
- Migration impact: no storage migration. `team:` task uses fail compilation
  with exact migration guidance.
- Tests: hidden-team rejection, role tool separation, handoff schemas, turn
  bounds, packaged inspection, selected-boundary retry/repair, and replay.
- Examples: two-role evidence collection and verification workflow.
- Live evidence: two-role OpenAI handoff plus deterministic verifier.
- Documentation: compiled replacement, migration, guarantees, and non-goals.
- Final disposition: redesigned. Each role is a task-local bounded agent with
  explicit provider, tools, limits, and structured output. Each handoff is an
  ordinary typed task output with sender, recipient, payload, task state,
  output digest, checkpoint, audit, and trace evidence. Typed routers provide
  explicit branching. Packaged scenario 39 verifies role-specific tool
  visibility, durable inspection, upstream handoff reuse during retry and
  repair, and effect-free replay. Free-form `team:` orchestration is rejected
  with migration guidance. The full local gate passes. Program state remains
  in progress until the bounded live handoff scenario executes.

### STR-001: End-to-end streaming

- Current behavior: fake, OpenAI, and Azure OpenAI agents can emit bounded
  durable provider progress while retaining normal final task output.
- User impact: long supported-provider calls expose inspectable progress in
  human or JSONL mode.
- Security or durability impact: event and transport caps, redaction,
  encrypted-capable storage, and final JSON isolation prevent unbounded deltas
  from corrupting automation.
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
- Final disposition: implemented with bounded live evidence pending. Provider
  fragments cross an awaited persistence boundary with monotonic task-attempt
  sequence numbers. SQLite schema 12 stores at most 256 events per task
  attempt and 4 KiB per payload; OpenAI SSE is capped at 8 MiB. Cancellation
  preserves accepted records. A dropped or malformed post-dispatch stream is
  uncertain and is not reconnected or resubmitted automatically. Terminal
  responses still pass ordinary finish-reason, usage, tool, structured-output,
  and task-output validation. Human progress uses stderr, JSONL emits
  versioned event envelopes plus the final outcome, and final JSON remains one
  document. Recorded replay copies source-linked events and performs zero
  effects. Focused provider/runtime tests and packaged CLI scenario 40 cover
  fragmentation, bounds, redaction, inspection, output isolation, and replay.
  The full local gate passes. Program state remains in progress until the
  bounded live OpenAI streaming scenario executes.

## Remote protocols, packs, and memory

### MCP-001: Safe MCP reconnect

- Current behavior: SQLite schema 13 persists MCP session generations and
  immutable call identities. Calls declared `pure`, `idempotent`, or `keyed`
  may reinitialize once, refresh `tools/list`, verify the selected schema
  digest, and redispatch. Unknown and at-most-once calls never redispatch after
  ambiguity.
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
- Final disposition: implemented. Protocol tests cover restart, one-reconnect
  bounds, unsafe refusal, stable and changed schemas, auth refresh, SSE,
  cancellation, and timeout. Packaged acceptance scenario 41 verifies a
  session-expiry reconnect, generation 2 inspection, durable reconnect event,
  successful call, and exactly two call attempts. Retry and replay preserve
  source-linked protocol records without effects.

### A2A-001: Safe remote-task continuation

- Current behavior: the call identity, remote task ID, latest task state, card
  generation, and submission ambiguity are durable before observation.
  `effects continue-remote` resumes a known task without `SendMessage`.
- User impact: a lost response can strand externally running work.
- Security or durability impact: blind `SendMessage` resubmission duplicates a
  remote task.
- Product decision: persist external task IDs before polling and resume polling
  or streaming; never resubmit an ambiguous task automatically.
- Required implementation: card refresh, interface compatibility, task ID and
  durable local stream sequence persistence with canonical `GetTask` fallback,
  auth refresh, artifact retrieval into ART-001, cancellation, bounded retry,
  and EFX-001 linkage.
- Migration impact: protocol task/session records.
- Tests: ambiguous submission, polling/stream resume, card/interface change,
  auth refresh, cancellation, artifacts, repair/retry/replay.
- Examples: resilient mock A2A workflow.
- Live evidence: deterministic local peer only.
- Documentation: continuation and reconciliation.
- Final disposition: implemented. Mock peers prove known-task continuation,
  ambiguous-send refusal, card refresh, bounded polling and streaming fallback,
  cancellation, same-origin interface and artifact policy, CAS ingestion, and
  auth refresh. Runtime repair tests prove the applied completed result is
  schema-validated and materialized while only descendants rerun. Packaged
  acceptance scenario 41 performs one `SendMessage`, resumes by task ID,
  retrieves an artifact, retries from the recovered boundary, and replays with
  zero effects.

### PACK-001: Deterministic pack resolution and lockfile

- Current behavior: manifest v1 resolves semantic constraints across contained
  local paths, full-commit Git sources, and digest-pinned tar-gzip archives.
  Lock v1 records the sorted concrete graph, source, compatibility, digest, and
  trust result. `--locked` rejects drift and `--offline` requires cache hits.
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
- Final disposition: implemented. Six focused CLI tests cover deterministic
  transitive resolution, constraints, conflicts, cycles, path containment,
  tamper, unreachable entries, pinned Git cache reuse, offline operation,
  immutable archive download and extraction bounds, and link rejection.
  Packaged acceptance scenario 42 verifies the checked-in two-pack graph through
  the public `packs verify-lock`, run, and replay paths.

### TRUST-001: Pack authenticity and trust policy

- Current behavior: every locked manifest has SHA-256 integrity and an explicit
  unsigned or Sigstore trust result. Optional verification checks the standard
  bundle, public-good trust root, certificate chain, identity, issuer,
  transparency proof, and timestamp. Unsigned policy is deny, warn, or allow.
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
- Final disposition: implemented. A public Cosign v3 fixture verifies
  successfully through the embedded Rust verifier, even though its short-lived
  certificate is now expired, because its signed timestamp proves validity at
  signing time. Tampered bytes, invalid timing evidence, an unallowlisted
  identity, and malformed bundles fail. Bundle digest and identity metadata are
  locked. Unsigned process packs are denied before loading unless the workflow
  explicitly acknowledges them.

### EXT-001: Isolated extension model

- Current behavior: the supported contracts are reviewed packs, MCP, and
  `extension.process`. The process protocol performs an exact version,
  schema, and capability handshake before a separately bounded invocation with
  a durable effect ID.
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
- Final disposition: redesigned and implemented. Native in-process libraries
  are rejected from the product surface. Three runtime tests cover successful
  negotiation, output validation, secret redaction, replay without execution,
  mismatch before invocation, output overflow, timeout, crash, and
  cancellation. Existing policy and process-tree tests cover direct argv,
  allowlists, environment selection, output capture, and termination. Packaged
  acceptance scenario 42 proves unsigned trust gating, one invocation, effect
  inspection, and effect-free replay.

### MEM-001: Optional semantic retrieval

- Current behavior: typed namespaced long-term memory supports exact reads,
  metadata-filtered text/vector/hybrid retrieval, explicit retention, and
  explicit promotion. SQLite and `local_hash` are built in; OpenAI embeddings
  and external memory adapters are optional.
- User impact: workflows can retrieve and selectively promote relevant prior
  entries without hidden model state.
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
- Final disposition: implemented and verified. Core, store, runtime, provider,
  compiler, and CLI tests cover typed/legacy entries, stable text and hybrid
  ranking, metadata filters, namespaces, expiry, corrupt dimensions, adapter
  validation, OpenAI request mapping and redaction, credential preflight,
  reindexing, repair refresh, and replay without adapter calls. Packaged
  acceptance scenario 43 executes the public local-hash example, CLI
  administration, selective retrieval repair, explicit promotion, and
  effect-free replay. No live embedding request is required because public
  examples remain deterministic and credential-free.

### PROV-001: Stateless tool continuation

- Current behavior: OpenAI/Azure tool agents support `store: false` with
  client-held, provider-neutral continuation.
- User impact: privacy-sensitive users can opt out of stored provider responses
  without losing tool-loop continuation.
- Security or durability impact: reasoning and function-call items are
  retained in order, and missing encrypted reasoning content fails closed.
- Product decision: persist provider-neutral opaque returned items needed for
  stateless continuation and replay them on the next request.
- Required implementation: versioned continuation items, provider mapping,
  size/redaction bounds, encryption under ENC-001, and capability negotiation.
- Migration impact: the existing provider-session format version 1 already
  represents provider-neutral conversations, so no database migration was
  required.
- Tests: multiple tools, reasoning items, cancellation, resume, repair session
  freshness, encrypted persistence, and malformed items.
- Examples: stateless OpenAI tool workflow.
- Live evidence: one packaged `store: false` OpenAI tool run.
- Documentation: stateful versus stateless continuation.
- Final disposition: implemented and verified. The OpenAI/Azure adapter
  automatically requests `reasoning.encrypted_content`, never sends
  `previous_response_id` in stateless mode, preserves returned item order and
  multiple call IDs, and rejects malformed unencrypted reasoning items.
  The 8 MiB stateless-input cap, 4 MiB provider-response cap, secret scrubbing,
  protected model-effect results, and protected provider-session columns
  supply the size, redaction, and ENC-001 boundaries.
  Compiler/provider/runtime tests cover
  negotiation, multiple calls, reasoning items, cancellation, malformed
  items, encrypted persistence, repair freshness, approval pause/resume, and
  effect-free replay. Credential-free acceptance scenario 4 compiles the
  stateless tool contract. The packaged GPT-5.6 run
  `run-019fa30a-0b7d-79b2-84de-0ee48fc369c7` completed two stateless model
  requests, one real tool call, and ordered call/result replay with 530 input
  and 33 output tokens. Credential-free replay
  `replay-019fa30a-7e3e-73b1-ba5c-5dd293dbbf33` produced identical output with
  zero effects, tool calls, and provider sessions. Ignored exact evidence is
  retained under `.release-evidence/openai-stateless-2026-07-27/`; the
  credential value is absent.

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

- Current behavior: exact/wildcard host grants compose with HTTP(S) scheme,
  effective-port, resolved-address, private-network, proxy, CA, connect-time,
  and response-size policy. Redirects and Unix sockets are disabled.
- User impact: public endpoints can be narrowed to HTTPS/443, while intentional
  local and internal peers require explicit private-network authority.
- Security or durability impact: direct clients validate the complete DNS
  answer and pin it before dispatch. Proxy routing is disabled by default;
  explicit proxy opt-in is documented as a trust delegation.
- Product decision: resolve and validate each destination against scheme, host,
  port, IP class, proxy, redirect, and TLS/CA policy at the adapter boundary.
- Required implementation: resolved-IP checks, private-range controls,
  rebinding defense, explicit proxy and Unix-socket denial, response limits,
  bounded DNS/connect setup, and protected custom-CA references.
- Migration impact: public HTTP(S) scheme defaults remain compatible. Private
  destinations now require `allowPrivate: true`; environment proxies require
  `allowProxy: true`.
- Tests: core policy and DSL tests cover credentials in URLs, schemes, ports,
  empty and mixed DNS answers, loopback/private/public IPv4 and IPv6, explicit
  private opt-in, limits, and custom-CA reference policy. Provider and protocol
  tests cover pinned synthetic DNS, redirect refusal, CA bundle success and
  failure, and policy-composed response limits.
- Examples: constrained MCP and A2A examples explicitly authorize localhost.
- Live evidence: the final public OpenAI matrix remains tracked separately; the
  security decisions here are proven without credentialed traffic.
- Documentation: network guide, policy, YAML, provider, security, threat model,
  limitations, architecture, and ADR 0019.
- Final disposition: implemented and verified by focused adapter/core tests,
  generated schema verification, and packaged CLI acceptance scenario 44,
  which denies a private destination before database creation or network I/O.

### ISO-001: Honest process isolation

- Current behavior: every shell and process-extension action exposes
  plan-visible `process` or `container` isolation. Existing actions default to
  bounded host-process mode, explicitly not a sandbox.
- User impact: users can select a portable stronger boundary without confusing
  executable policy with isolation.
- Security or durability impact: host mode retains the agentctl identity.
  Container mode uses a local digest-pinned image with fixed read-only,
  networkless, non-root, capability-dropped, resource-bounded invocation.
- Product decision: require an explicit isolation mode. `process` is the honest
  host mode; `container` is the portable strong boundary. Optional platform
  backends can be added when detected, but no weak emulation is claimed.
- Required implementation: implemented DSL/plan visibility, fail-closed
  engine/image preflight, memory/CPU/PID/output/time limits, forced named
  container cleanup, protected engine environment, direct entrypoint, and
  explicit unsupported native-backend documentation.
- Migration impact: existing actions default to documented host-process mode.
- Tests: 61 core tests and 8 focused process tests cover defaults, invalid
  combinations, plan visibility, engine-safe environment, working directory,
  process tree, output/time/cancellation, cleanup construction, and container
  resource/security flags. Packaged CLI acceptance scenario 45 proves a
  requested missing engine fails before process effect dispatch.
- Examples: `examples/v1/process-isolation.yaml` contains explicit host and
  content-addressed container actions.
- Live evidence: the OCI gate now invokes a real action inside the exact
  repository image. Local execution is pending because Podman 5.8.2/libkrun
  repeatedly lost `gvproxy`, leaving both forwarding endpoints unavailable
  before image build.
- Documentation: process-isolation guide, policy, YAML, DSL, security, threat
  model, limitations, architecture, container contract, terminology, and ADR
  0020.
- Final disposition: implementation and deterministic evidence complete;
  actual container execution remains environment-blocked and therefore this
  entry is not yet marked verified.

### BUD-001: Enforceable resource and cost budgets

- Current behavior: the compiled plan carries optional run-wide request, turn,
  tool, token, wall-time, process-output, artifact, task, expansion, loop, and
  monetary limits.
- Security and durability: SQLite atomically reserves known units before fresh
  provider, tool, process, or artifact dispatch. Actual usage is reconciled
  afterward. The wall deadline uses the durable run creation timestamp.
- Cost decision: token-only limits require no price. Monetary limits require
  authoritative provider cost or explicit versioned integer custom pricing
  keyed by `provider/model`; unknown cost is exposed and never fabricated.
- Migration: schema 15 adds run budget and idempotent reservation records.
  Budget snapshots are present in checkpoints, audit, and CLI inspection.
- Evidence: compiler graph-count/custom-pricing tests, every dynamic dimension
  and exact-bound test, parallel reservation race test, actual-overrun runtime
  test, provider/tool/process/artifact pre-dispatch termination tests, wall
  cancellation and paused-resume tests, pricing-class reconciliation,
  retry/repair/replay accounting, public example, acceptance scenario 46, and
  the complete 12-stage deterministic gate.
- Live evidence: `cargo xtask resource-budget-live-openai` passed through the
  packaged CLI with one `gpt-5.6` dispatch, 18 input tokens, 5 output tokens,
  no tool calls, and a durable denial before attempted request 2.
- Final disposition: implemented and verified.

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
