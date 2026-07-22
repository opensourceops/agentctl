# Threat model

## Assets and boundaries

Assets are workspace files, allowed environment secrets, provider accounts, external systems reached by tools, workflow history, prompts/results, approvals, and the integrity of deterministic scheduling. Boundaries are the YAML/pack parser, filesystem/process/network executors, provider APIs, MCP servers, A2A peers, SQLite, trace exporters, and dependencies.

The local operator and reviewed binary are trusted. Workflow authors are only as trusted as policy grants. Models, file content, remote descriptions/results, pack content without independent provenance, and all network peers are untrusted. The host OS, CA store, and Rust dependency supply chain are assumed but monitored dependencies.

| Threat | Control | Residual risk |
| --- | --- | --- |
| Malicious YAML/template causes code execution or resource exhaustion | strict fields, constrained paths/equality, 1 MiB bound, fuzzing | deeply nested valid data remains bounded mainly by parser behavior |
| Path traversal or symlink escape | canonical roots and focused tests | TOCTOU is possible if another process swaps paths; isolate hostile workspaces |
| Secret exfiltration through CLI/log/database/trace | env references, no key flags, allowlists, redaction, secret scan | authorized tools can deliberately transmit permitted data |
| Command injection | direct argv, no shell, cleared env, executable allowlist | an allowed executable may interpret malicious arguments |
| SSRF/redirect bypass | URL parse, host allowlist, disabled redirects, tests | DNS/proxy behavior needs external network containment for hostile inputs |
| Prompt injection grants tool authority | policy outside model, visible tool set, schema validation, approvals | an operator may approve deceptive content |
| MCP annotation or A2A card claims safety | always treated as untrusted metadata | compromised authorized peer can return malicious but schema-valid data |
| Crash duplicates an external mutation | request-before-start ledger, uncertain state, no silent retry | external action may have happened without acknowledgement |
| Replay reissues effects | recorded replay uses stored terminal output only | replayed data may no longer reflect current reality, by design |
| Approval bypass in CI | non-interactive durable pause or explicit deny/fail; operator resolution | stolen database write access is outside application trust boundary |
| Pack substitution | SHA-256 verification and semver/API checks | digest source/signature trust is manual |
| Corrupt or future state misexecutes | schema/version/checksum/deserialization failures | SQLite file deletion or rollback by an attacker is not prevented |
| Dependency compromise | locked registry-only deps, cargo-deny, license/source checks | registry compromise and zero-days remain possible |

No unresolved critical or high-severity defect is knowingly accepted for the implemented boundary. Deferred sandboxing, signature verification, distributed concurrency, and encrypted storage are explicit product limitations, not implied controls.
