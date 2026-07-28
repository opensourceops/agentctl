# Definition of done evidence map

Status values distinguish **deterministically tested**, **mock-provider
tested**, **live OpenAI tested**, **operationally tested**,
**syntax-validated only**, and **explicit non-goal**.

| Area | Status | Evidence |
| --- | --- | --- |
| Strict YAML, diagnostics, compiled plan, inputs/outputs | deterministically tested | core tests; generated schema; acceptance validation/plan/input scenarios |
| Rust-only CLI, clean install/package, no Node dependency | operationally tested | source install; packaged/copy isolation; quickstart; production boundary gate |
| Deterministic actions/state/effects/checkpoints/audit/traces | deterministically tested | runtime/store tests and public inspect acceptance |
| Fake agent and strict tool continuation | mock-provider tested | tool-using acceptance with artifact and durable evidence |
| GPT-5.6 workflow runtime | live OpenAI tested | 27-request packaged macOS arm64 and native Linux arm64 matrix covered tools, orchestration, recovery, replay, streaming, and budgets |
| OpenAI reasoning/context/storage/cache/strict schemas/multiple calls/usage mapping | deterministic mapping tests plus final live evidence | provider mapping tests, compiler rejection tests, final live continuation and usage |
| Replay without credentials or network | deterministically and operationally tested on exact live state | panic-on-call provider/tool regression; OCI `--network none` replay with identical output/artifact digest, zero fresh effects/tool calls, and source-effect audit links |
| Resume/reject/uncertainty/fork/retry/auth/rate-limit/malformed/cancellation semantics | deterministically tested | focused provider/runtime/store tests and acceptance scenarios |
| Non-interactive approvals, cron, inputs, timeout, SIGTERM | operationally tested | empty-environment and signal acceptance; operations guide |
| OCI non-root/read-only/mount/JSON/artifact/state contract | operationally tested | current-source Linux arm64 binary passed mock/failure/signal cases; earlier exact live-state replay ran as UID/GID 65532 |
| Image high/critical scan and SBOM | locally and hosted operationally tested | current secret-CA build and OCI suite; checksum-verified Trivy 0.72.0 policy gate; valid retained CycloneDX artifacts with digests |
| Linux x64, hosted macOS arm64, hosted Windows x64 | hosted operationally tested | exact-head full gates and packages execute on standard GitHub runner labels |
| Anthropic/Google/Azure adapters; MCP/A2A | mock-provider/protocol tested | native mapping/protocol tests; not live-tested |
| Advisories/licenses/sources/secrets/actions | deterministically and locally security-tested | cargo-deny; metadata/source; deterministic scan; Gitleaks complete history/tree and synthetic detection; full-SHA action-pin check; actionlint |
| Parallel/dynamic orchestration, packs/trust, semantic memory, and selected-field encryption | deterministically, operationally, and selectively live tested | framework completeness acceptance, public examples, native Linux arm64 OCI acceptance, and live GPT-5.6 matrix |
| Distributed execution, hosted service/registry/UI, and event scheduling | explicit non-goal | `docs/LIMITATIONS.md`, `docs/execution/LIMITATION_BURNDOWN.md`, ADR 0005/0006/0007 |
| No known P0/P1 correctness/security defect in implemented boundary | locally and hosted verified | independent framework review, credential-free exact-head gates, bounded live matrix, and local/hosted image security evidence |
