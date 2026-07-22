# Definition of done evidence map

Status values distinguish **deterministically tested**, **mock-provider tested**, **live OpenAI tested**, **operationally tested**, **syntax-validated only**, and **deferred/non-goal**.

| Area | Status | Evidence |
| --- | --- | --- |
| Strict YAML, diagnostics, compiled plan, inputs/outputs | deterministically tested | core tests; generated schema; acceptance validation/plan/input scenarios |
| Rust-only CLI, clean install/package, no Node dependency | operationally tested | source install; packaged/copy isolation; quickstart; production boundary gate |
| Deterministic actions/state/effects/checkpoints/audit/traces | deterministically tested | runtime/store tests and public inspect acceptance |
| Fake agent and strict tool continuation | mock-provider tested | tool-using acceptance with artifact and durable evidence |
| GPT-5.6 tool-using runtime | prior live OpenAI evidence reviewed | packaged local and OCI live acceptance was recorded by the preceding run; not called again in the final audit |
| OpenAI reasoning/context/storage/cache/strict schemas/multiple calls/usage mapping | deterministic mapping tests plus prior live evidence | provider mapping tests, compiler rejection tests, prior live continuation |
| Replay without credentials or network | deterministically and operationally tested | provider/tool-executor regression; host replay; OCI `--network none` replay with identical output and zero effects/tool calls |
| Resume/reject/uncertainty/fork/retry/auth/rate-limit/malformed/cancellation semantics | deterministically tested | focused provider/runtime/store tests and acceptance scenarios |
| Non-interactive approvals, cron, inputs, timeout, SIGTERM | operationally tested | empty-environment and signal acceptance; operations guide |
| OCI non-root/read-only/mount/JSON/artifact/state contract | operationally tested; prior OpenAI live evidence | final native arm64 mock, failure, signal, and offline-replay cases; prior OpenAI image run |
| Image high/critical scan and SBOM | operationally tested on arm64 | Trivy result and CycloneDX artifact recorded in verification ledger |
| Linux amd64 image and external CI/vendor pipelines | syntax/configuration validated only | GitHub job and pipeline examples; not remotely dispatched here |
| Anthropic/Google/Azure adapters; MCP/A2A | mock-provider/protocol tested | native mapping/protocol tests; not live-tested |
| Advisories/licenses/sources/secrets | deterministically tested | cargo-deny, metadata, source, and secret gates |
| Parallel/dynamic orchestration, pack ecosystem, vector/encrypted/distributed additions | deferred or non-goal | `docs/LIMITATIONS.md`, ADR 0005/0006/0007 |
| No known P0/P1 correctness/security defect in implemented boundary | verified for internal review | canonical gates, clean-room acceptance audit, image scan, conservative documented limits |
