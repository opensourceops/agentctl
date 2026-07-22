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
- Packs require a supported manifest/version and can be checked against SHA-256 integrity.
- The workspace forbids unsafe Rust, denies warnings, locks dependencies, checks licenses/sources/advisories, scans secret patterns, and keeps live tests outside CI.

## Limitations

Path and executable allowlists are not a sandbox. A permitted program can access anything the operating-system identity can access. Host allowlists do not defend against every DNS rebinding, proxy, local-service, or compromised endpoint scenario; use network isolation for hostile workflows. SHA-256 integrity establishes sameness, not author identity. SQLite protects local correctness but is not encrypted and is not a secret store.

Prompts, file content, model output, remote artifacts, and tool output may be confidential or malicious. Treat them as data, validate before mutation, minimize trace export, and isolate untrusted automation. Workflow, input, pack, direct-read, existing-write-target, and instruction files are capped at 1 MiB. Approval is a decision point, not proof that an operation is safe. At-most-once recovery may leave an uncertain external outcome for human reconciliation.

MCP reconnection and A2A resubmission are intentionally not automatic. Streaming is bounded but completed results, not token deltas, enter workflow state. Windows cannot express Unix database mode bits; rely on the user profile ACL and CI tests.

Report vulnerabilities privately to the repository maintainer. Do not include credentials, database contents, or production prompts in a report.
