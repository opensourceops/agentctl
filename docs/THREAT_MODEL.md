# Threat model

## Assets and boundaries

Assets are workspace files, content-addressed artifact bytes, referenced
environment/file/process secrets, provider accounts, external systems reached
by tools, workflow history, prompts/results, approvals, and the integrity of
deterministic scheduling. Boundaries are the YAML/pack parser, secret resolver,
filesystem/process/network executors, provider APIs, MCP servers, A2A peers,
SQLite and its sibling artifact root, trace exporters, and dependencies.

The local operator and reviewed binary are trusted. Workflow authors are only as trusted as policy grants. Models, file content, remote descriptions/results, pack content without independent provenance, and all network peers are untrusted. The host OS, CA store, and Rust dependency supply chain are assumed but monitored dependencies.

| Threat | Control | Residual risk |
| --- | --- | --- |
| Malicious YAML/template causes code execution or resource exhaustion | strict fields, constrained paths/equality, 1 MiB bound, fuzzing | deeply nested valid data remains bounded mainly by parser behavior |
| Path traversal or symlink escape | canonical roots and focused tests | TOCTOU is possible if another process swaps paths; isolate hostile workspaces |
| Secret exfiltration through CLI/log/database/trace | typed references, canonical file roots, bounded allowlisted process helpers, zeroizing values, no key flags, redaction, raw-database tests, secret scan | authorized recipients can deliberately transmit or transform permitted data |
| SQLite disclosure reveals confidential run content | optional AES-256-GCM field envelopes, external key reference, authenticated context, fail-closed triggers, transactional rotation | metadata and artifact bytes remain visible; unencrypted and pre-migration backups remain sensitive |
| Command injection | direct argv, no shell, cleared env, executable allowlist | an allowed executable may interpret malicious arguments |
| Host process exceeds intended authority | explicit plan-visible `process` versus `container` mode; container mode is digest-pinned, local-only, networkless, read-only, non-root, capability-dropped, resource-bounded, and fail-closed | host mode and secret helpers retain the agentctl identity; container engine/image are trusted; readable workspace and explicit secrets remain visible inside the container |
| SSRF, DNS rebinding, or redirect bypass | strict HTTP(S) scheme/host/port policy, every-answer IP classification, direct-client DNS pinning, private-network deny by default, redirects and Unix sockets disabled, proxies denied by default, bounded responses | explicit proxy opt-in trusts that proxy; authorized or compromised destinations remain data recipients; external egress isolation is still stronger |
| Unreviewed remote pack acquisition reaches an unwanted source | immutable exact source, HTTPS except loopback fixtures, no URL credentials/query/fragment, redirects disabled, digest and extraction bounds, locked offline execution option | runtime network policy does not authorize configuration-time pack fetches; use `--locked --offline` for untrusted workflows and prepare the cache through a trusted operator path |
| Prompt injection grants tool authority | policy outside model, visible tool set, schema validation, approvals | an operator may approve deceptive content |
| MCP annotation or A2A card claims safety | always treated as untrusted metadata | compromised authorized peer can return malicious but schema-valid data |
| Crash duplicates an external mutation | request-before-start ledger, uncertain state, no silent retry | external action may have happened without acknowledgement |
| Replay reissues effects | recorded replay uses stored terminal output only | replayed data may no longer reflect current reality, by design |
| Repair reuses tampered or unrelated state | stable workflow identity, versioned task/input/contract/output/state fingerprints, immutable CAS blobs, artifact digest checks, transactional materialization | an attacker with database/artifact-root write access is inside the local application trust boundary |
| Repair duplicates a partial mutation | closure effect inspection, conservative uncertainty block, narrow operator `not-applied` reconciliation | remote truth may remain unknowable and keep the repair blocked |
| Repair carries failed model state | every repaired agent starts a fresh provider session; dataflow uses validated JSON output | a valid reused output can still contain hostile content and must remain policy constrained |
| Source or workspace deletion breaks repair | reused output/state metadata and independent CAS references are materialized into the repair run | deleting/corrupting the shared CAS or restoring SQLite without it still blocks repair |
| Concurrent GC removes an in-flight artifact | cross-process lock, durable ingestion leases, transactional references, quarantine recovery | network filesystems with broken advisory-lock semantics are unsupported |
| Approval bypass in CI | non-interactive durable pause or explicit deny/fail; operator resolution | stolen database write access is outside application trust boundary |
| Pack substitution | SHA-256 verification and semver/API checks | digest source/signature trust is manual |
| Corrupt or future state misexecutes | schema/version/checksum/deserialization failures | SQLite file deletion or rollback by an attacker is not prevented |
| Dependency compromise | locked registry-only deps, cargo-deny, license/source checks | registry compromise and zero-days remain possible |

No unresolved critical or high-severity defect is knowingly accepted for the implemented boundary. Host process policy, native OS sandboxing, signature verification, and distributed concurrency are not implied controls. Container isolation is claimed only for actions that explicitly request and successfully preflight that mode. State encryption protects its documented columns only and is not described as full-database encryption.

Run access control is the database file and operating-system identity. `agentctl` has no multi-tenant authorization layer; do not let an untrusted principal select another tenant's source run from a shared database.
