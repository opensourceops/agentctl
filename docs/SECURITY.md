# Security

## Controls

- Workflow parsing is strict, bounded to 1 MiB, source-aware, and has no executable expression language.
- Environment-backed primary credentials are resolved immediately before provider dispatch; custom header references are resolved while constructing the adapter, before a run or database is created. There are no API-key flags. Provider/protocol response JSON keys and values, provider request IDs, errors, subprocess output, and traces redact every known configured secret value before persistence or output.
- Canonical read/write roots reject `..` and symlink escape. Writes use temporary files and rename.
- Processes require an allowed executable basename, direct argv, cleared environment, selected variables, validated output/timeout bounds, concurrent stdout/stderr draining, and cancellation. Output-limit, timeout, and cancellation paths terminate and reap the child; diagnostics are bounded and omit captured output when secret environment values are present.
- Network destinations require an exact/wildcard host grant. Provider and protocol clients disable redirects and use rustls.
- Tool input and output JSON Schemas are enforced. Models, MCP annotations, A2A cards, remote schemas, and results cannot grant capabilities.
- Requests are ledgered before effects. Global denial or approval cannot be weakened by a tool contract. Approval is durable; non-interactive mode pauses with exit `3` or uses an explicitly stricter deny/fail mode, never a prompt or implicit approval.
- SQLite uses foreign keys, WAL/busy timeout, version checks, checksummed checkpoints, and mode `0600` on Unix.
- Optional application-level state encryption uses versioned AES-256-GCM envelopes with per-value random nonces, field-bound authenticated data, key IDs, environment references, transactional migration/rotation, and database triggers that reject plaintext or stale-key writes after enablement. Missing, wrong, unsupported, or tampered keys/envelopes fail closed.
- Repair never mutates a terminal source. Reuse requires versioned definition/input/contract/output/state metadata and verified content-addressed artifact sizes and SHA-256 digests. Artifact ingestion uses atomic no-clobber writes, immutable blobs, bounded leases, and a cross-process GC lock. Repair creation and reused-task/reference materialization are one SQLite transaction.
- A recorded replay cannot be a repair source because it has no direct effect ledger. A materialized reused/recorded task cannot be selected for restart without returning to direct effect history. Repaired agents start fresh provider sessions.
- Packs require a supported manifest/version and can be checked against SHA-256 integrity.
- The workspace forbids unsafe Rust, denies warnings, locks dependencies, checks licenses/sources/advisories, scans secret patterns, and keeps live tests outside CI.

## Limitations

Path and executable allowlists are not a sandbox. A permitted program can access anything the operating-system identity can access. Host allowlists do not defend against every DNS rebinding, proxy, local-service, or compromised endpoint scenario; use network isolation for hostile workflows. SHA-256 integrity establishes sameness, not author identity. State encryption is application-level selected-field protection, not full-database encryption, access control, or a secret store.

Prompts, file content, model output, remote artifacts, and tool output may be confidential or malicious. Treat them as data, validate before mutation, minimize trace export, and isolate untrusted automation. Workflow, input, pack, direct-read, existing-write-target, and instruction files are capped at 1 MiB. Approval is a decision point, not proof that an operation is safe. At-most-once recovery may leave an uncertain external outcome for human reconciliation.

MCP reconnection and A2A resubmission are intentionally not automatic. Streaming is bounded but completed results, not token deltas, enter workflow state. Windows cannot express Unix database mode bits; rely on the user profile ACL and CI tests.

SQLite and sibling artifact-root access are the repair authorization boundary. There is no tenant identity or row-level authorization. Run IDs, task/effect identity, status, timing, digests, paths, sizes, schema metadata, and key references remain visible. Artifact blob bytes are not encrypted. An identity that can modify the state directory can corrupt or replace local history, although envelope authentication and artifact digest verification prevent silent use of changed protected content.

Report vulnerabilities privately to the repository maintainer. Do not include credentials, database contents, or production prompts in a report.
