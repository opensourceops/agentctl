# Security

## Controls

- Workflow parsing is strict, bounded to 1 MiB, source-aware, and has no executable expression language.
- Provider credentials in the fresh execution closure are preflighted before a
  new run record or effect is created; custom header references are resolved
  while constructing a required adapter; action environment references are
  resolved at the task boundary. References may use an environment variable, a
  bounded canonical file under `secretFileRoots`, or a direct bounded process
  under `secretProcessAllowlist`. Values use zeroizing memory wrappers and never
  enter ordinary persisted state. There are no API-key flags.
  Provider/protocol response JSON keys and values, provider request IDs, errors,
  subprocess output, and traces redact every known configured secret value
  before persistence or output.
- Canonical read/write roots reject `..` and symlink escape. Writes use temporary files and rename.
- Processes require an allowed executable basename and explicit isolation
  mode. Default `process` mode uses direct host arguments, a cleared
  environment, selected variables, validated output/timeout bounds, concurrent
  stdout/stderr draining, cancellation, and process-tree termination, but is
  not a sandbox. `container` mode requires a local digest-pinned image and
  available Docker or Podman engine. It disables pulls/networking, fixes a
  read-only root/workspace, runs non-root, drops capabilities, enables
  `no-new-privileges`, and bounds memory/CPU/PIDs/output/time. Engine/image
  preflight fails without host fallback; abnormal exits trigger forced named
  container cleanup.
- Required provider and protocol destinations are authorized before run
  persistence. Scheme, effective port, and host must be granted. Every resolved
  address is checked against private/reserved policy and the complete accepted
  answer is pinned into the direct HTTP client, preventing a second DNS lookup.
  Empty or mixed public/private answers fail closed. Redirects and Unix sockets
  are disabled. Environment proxies are ignored by default. TLS uses rustls;
  optional custom CA bundles are certificate-only protected references.
  DNS/connect time, protocol operation time, task time, and response bytes are
  bounded.
- Tool input and output JSON Schemas are enforced. Models, MCP annotations, A2A cards, remote schemas, and results cannot grant capabilities.
- Requests are ledgered before effects. Global denial or approval cannot be weakened by a tool contract. Approval is durable; non-interactive mode pauses with exit `3` or uses an explicitly stricter deny/fail mode, never a prompt or implicit approval.
- SQLite uses foreign keys, WAL/busy timeout, version checks, checksummed checkpoints, and mode `0600` on Unix.
- Optional application-level state encryption uses versioned AES-256-GCM envelopes with per-value random nonces, field-bound authenticated data, key IDs, environment references, transactional migration/rotation, and database triggers that reject plaintext or stale-key writes after enablement. Missing, wrong, unsupported, or tampered keys/envelopes fail closed.
- Repair never mutates a terminal source. Reuse requires versioned definition/input/contract/output/state metadata and verified content-addressed artifact sizes and SHA-256 digests. Artifact ingestion uses atomic no-clobber writes, immutable blobs, bounded leases, and a cross-process GC lock. Repair creation and reused-task/reference materialization are one SQLite transaction.
- A recorded replay cannot be a repair source because it has no direct effect ledger. A materialized reused/recorded task cannot be selected for restart without returning to direct effect history. Repaired agents start fresh provider sessions.
- Pack graphs are content-locked. Local paths remain contained; Git commits are
  fully pinned; immutable archives are redirect-free, digest-checked, and
  extraction-bounded. Optional Sigstore bundles verify an allowlisted identity
  and issuer against the embedded public-good trust root. Unsigned process packs
  cannot load without an explicit review acknowledgement.
- Process extensions negotiate an exact version, schemas, and capabilities
  before invocation. They use direct argv, cleared selected environment,
  bounded input/output/time, process-tree cancellation, a durable effect
  identity, and secret redaction.
- Long-term memory uses typed bounded entries, exact metadata filters, explicit
  retention, bounded result/candidate counts, finite dimension-checked vectors,
  and recorded retrieval effects. External adapter results are validated before
  entering task state. Embedding credentials use the same secret resolution
  and redaction boundary as model providers.
- The workspace forbids unsafe Rust, denies warnings, locks dependencies, checks licenses/sources/advisories, scans secret patterns, and keeps live tests outside CI.

## Limitations

Path and executable allowlists are not a sandbox. A permitted program, including
a secret helper, can access anything the operating-system identity can access.
Container-isolated actions can read their mounted working directory and
receive explicitly authorized secrets; the selected image and container engine
remain trusted dependencies. No native Linux namespace, macOS sandbox-profile,
or Windows restricted-token backend is claimed.
Redaction cannot prevent an authorized recipient from transforming a secret
before exfiltration. Enabling `allowProxy` explicitly delegates routing and
name resolution to the configured environment proxy, so treat that proxy as a
trusted network boundary. An authorized or compromised endpoint can still
exfiltrate data it legitimately receives. Use external egress isolation for
hostile workflows. SHA-256 integrity establishes sameness; Sigstore
identity depends on the configured issuer, subject, bundle evidence, and
installed trust root. State encryption is application-level selected-field protection, not
full-database encryption, access control, or a secret store.

Prompts, file content, model output, remote artifacts, and tool output may be confidential or malicious. Treat them as data, validate before mutation, minimize trace export, and isolate untrusted automation. Workflow, input, pack, direct-read, existing-write-target, and instruction files are capped at 1 MiB. Approval is a decision point, not proof that an operation is safe. At-most-once recovery may leave an uncertain external outcome for human reconciliation.

MCP reconnects at most once only for calls declared `pure`, `idempotent`, or `keyed`, and verifies the refreshed tool schema before redispatch. Unknown and at-most-once MCP calls are never automatically repeated after ambiguity. A2A persists a known remote task ID and may resume observation, but never resubmits an ambiguous `SendMessage`. Streaming is bounded but completed results, not progress deltas, enter workflow state. Windows cannot express Unix database mode bits; rely on the user profile ACL and CI tests.

SQLite and sibling artifact-root access are the repair authorization boundary. There is no tenant identity or row-level authorization. Run IDs, task/effect identity, status, timing, digests, paths, sizes, schema metadata, and key references remain visible. Artifact blob bytes are not encrypted. An identity that can modify the state directory can corrupt or replace local history, although envelope authentication and artifact digest verification prevent silent use of changed protected content.

Memory keys, namespaces, format versions, timestamps, expiry, embedding provider,
and dimensions remain operational metadata. Entry payloads, searchable text,
metadata JSON, and embedding vectors are protected when selected-field state
encryption is enabled. The local hash provider is deterministic lexical
indexing and must not be described as a confidential local neural model.

Report vulnerabilities privately to the repository maintainer. Do not include credentials, database contents, or production prompts in a report.
