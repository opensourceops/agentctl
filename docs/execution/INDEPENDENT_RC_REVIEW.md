# Independent release-candidate review

> Point-in-time review snapshot. Its residual-risk and recommendation sections describe the reviewed baseline before the final bounded-process and hosted-CI hardening. Current status: [HOSTED_CI_PREPARATION.md](HOSTED_CI_PREPARATION.md) and [STATUS.md](STATUS.md).

Review date: 2026-07-22 (Asia/Kolkata)

Recommendation: **Ready for internal review**.

The reviewed baseline was `b4e96dbebd81b1f3eb844d6c0668952c691677d9`, five commits ahead of `origin/main` (`be9d0ae`). The tracked tree was initially clean. This review found and remediated one P0, five P1s, and five scoped P2s. No known P0/P1 implementation defect remains in the declared local, scheduled, or generic OCI runtime boundary.

The branch is not yet a `v1alpha1` release candidate because none of its Rust CI workflows or release-prep jobs exists on the remote default branch, no PR exists, and GitHub reports zero workflow runs. The default local OCI build also could not complete after source changes because this host's container trust store rejects the intercepted certificates for Rust/crates.io. A current Linux arm64 binary was independently built from the reviewed source with networking disabled and passed every OCI runtime scenario in the production distroless image, but that is not equivalent to a green committed image-build job.

## Hosted CI

| Evidence | Actual state |
| --- | --- |
| GitHub workflows | Local YAML only; configured but not pushed/dispatched |
| PR checks | No PR and no checks |
| Linux amd64 | Not executed |
| macOS | Not executed on GitHub; local arm64 verification passed |
| Windows | Not executed |
| Formatting / Clippy / tests | Passed locally; not executed on GitHub |
| Acceptance / packaging | Passed locally; not executed on GitHub |
| Container / Trivy / SBOM | Runtime scenarios passed locally on Linux arm64; current default build and current scan/SBOM not completed |
| Branch protection | GitHub reports `main` is unprotected |

## Findings and remediation

| ID | Severity | Finding, impact, and root cause | Remediation and regression |
| --- | --- | --- | --- |
| RC-001 | P0 | Provider and MCP/A2A endpoints could echo environment-backed custom-header credentials into successful JSON, reasoning/tool payloads, or provider request-ID/error fields. Only the primary provider credential was scrubbed, and arbitrary JSON keys were not covered. This could persist a credential in effects, task output, audit-visible state, or CLI output. | Provider and protocol responses now recursively redact every configured credential/header value from JSON keys and values; provider error messages and request IDs are also bounded and redacted. Mock tests cover successful and failed provider responses plus MCP structured output. |
| RC-002 | P1 | A tool contract's `approval: never` or `always` replaced the global policy decision, so it could bypass a global deny or required approval. Provider allowlists compared the adapter kind instead of the workflow provider name, while tool allowlists were also applied to model calls. | Global denial/approval now wins before contract-specific approval. Provider and tool allowlists are independent and provider policy uses the compiled provider key. Unit/runtime tests prove denial cannot be bypassed and a named provider allowlist succeeds. |
| RC-003 | P1 | Gemini tool continuation used a hardcoded function-response name, generated weak fallback IDs, and discarded Gemini 3 thought signatures. The advertised Google tool path could therefore fail or correlate the wrong call. | Function results map to the originating name, fallback IDs are response-scoped, and `thoughtSignature` is stored as provider metadata and returned unchanged. The mock continuation regression follows Google's required function identity and thought-signature flow. |
| RC-004 | P1 | The compiler merged agent/task `vars`, but the runtime always evaluated inputs with an empty variable map. Documented `${{ vars.* }}` expressions failed at execution. | Task variables are rendered from input/memory/dependency context before task inputs. A runtime regression covers agent defaults, task overrides, inputs, and dependency outputs. |
| RC-005 | P1 | Cancellation during retry backoff returned cancellation without terminalizing the durable run and tasks. The CLI could exit while SQLite still said `running`. | The backoff cancellation branch now uses the same durable cancellation transition as in-flight cancellation. A regression asserts both run and task are `cancelled`. |
| RC-006 | P1 | Recorded replay always exited `0`, even when the terminal source was `failed` or `cancelled`. Schedulers could accept a reconstructed failure as success. | Replay now uses the common outcome-to-exit mapping (`0`, `3`, `4`, `130`). Public acceptance replays a failed run and requires exit `4`. |
| RC-007 | P2 | Tool effect completion and tool-call completion were separate SQLite transactions. A crash could expose conflicting terminal ledger records. | Both success/failure and uncertainty updates now commit effect and tool-call rows atomically. The regression deliberately fails the second update and proves the first rolls back. |
| RC-008 | P2 | Anthropic thinking/redacted-thinking blocks were discarded before a tool continuation. | Opaque reasoning blocks are preserved and returned; a native mock regression verifies the signed thinking block round trip. |
| RC-009 | P2 | CLI text files, direct read actions, existing write targets, and agent instruction files used unbounded reads despite the stated 1 MiB parser/tool limit. | All those paths now use bounded readers and fail before retaining oversized content. CLI and runtime regressions use 1 MiB + 1 fixtures and confirm validation/durable failure. |
| RC-010 | P2 | Write-path canonicalization checked only the immediate parent, so safe nested new paths below a writable root were rejected even though atomic write creates their parents. | Canonicalization now walks to the nearest existing ancestor while preserving symlink containment. Unit coverage includes nested missing paths and symlink escape. |
| RC-011 | P2 | The OCI source label named the wrong repository, the builder unnecessarily re-synced its installed Rust toolchain, and build failures hid Podman's stdout. | The source label is corrected, `RUSTUP_TOOLCHAIN=1.88.0` selects the image's installed toolchain, and both build output streams are reported. The remaining crates.io certificate failure is environmental and explicitly unresolved. |

Google's own documentation requires a function response to carry the matching function name/ID and requires Gemini 3 thought signatures on subsequent function-call turns: [function calling](https://ai.google.dev/gemini-api/docs/generate-content/function-calling) and [thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures).

## Independently confirmed guarantees

| Guarantee | Production path | Executable evidence | User journey / documentation |
| --- | --- | --- | --- |
| Stable graph order | compiler topological order with declaration-order tie break | compiler unit tests | acceptance 1; DSL docs |
| Explicit terminal state machine | core run/task transition tables | exhaustive state tests | acceptance failure/cancel/approval cases |
| Transactional state/checkpoint/audit | SQLite immediate transactions | store transition/checkpoint tests | inspect across acceptance runs |
| Effect recorded before dispatch | runtime `prepare_effect` then `mark_effect_started` | effect identity/recovery tests | approval/resume and timeout scenarios |
| No duplicate confirmed effects on resume | durable effect identity and confirmed-result reuse | runtime/store tests | acceptance 10 and 12 |
| Uncertain effects block resume | started/uncertain handling | timeout and cancellation tests | acceptance 15 |
| Recorded replay dispatches nothing | terminal-output reconstruction path | panic-on-provider/tool regression | acceptance 13 and retained OpenAI database replay |
| Fork creates fresh effects | fork creates a linked execute run | counting-provider test | acceptance 14 |
| Approval persistence | SQLite approvals and policy engine | policy/runtime tests | acceptance 9–12 |
| Cancellation durability | token/flag plus terminal transitions | in-flight and retry-backoff tests | acceptance 22 and OCI SIGTERM runtime case |
| Strict tool schemas | compiler and runtime contract validators | malformed input/output tests | acceptance 6–7 |
| Secret redaction | provider/protocol/runtime recursive redaction | provider, MCP, subprocess, trace tests | security docs; no live secret retained |
| Capability negotiation | compiler provider capability sets | compiler tests | acceptance 3–4 and `providers inspect` |
| CLI machine contract | versioned envelope and outcome exit mapping | CLI tests | acceptance 13 and 18 |
| Pack integrity | canonical containment plus SHA-256 verification | pack tests | reusable-pack example |

The retained GPT-5.6 database was independently hash-checked and replayed again with the newly packaged CLI under `env -i`. It produced the same declared output, a distinct replay run, and zero effects, tool calls, or provider sessions. No new OpenAI request was made: the changed provider continuation/redaction paths have direct mock coverage, while recorded replay was exercised keylessly.

## Provider and protocol support

| Adapter | Evidence | Review classification |
| --- | --- | --- |
| Fake | runtime and public acceptance | Executed and passed |
| OpenAI Responses | native mock mapping plus retained prior GPT-5.6 tool run; current keyless replay | Live evidence retained; no new live call |
| Azure OpenAI | request/auth/path/response mocks | Mock-tested only |
| Anthropic | native text/tool/usage/thinking continuation mocks | Mock-tested only |
| Google Gemini | native content/function/usage/signature continuation mocks | Mock-tested only |
| MCP | initialize/session/list/call/version/timeout/redaction mocks | Mock-tested only |
| A2A | discovery/interface/origin/send/poll/cancel mocks | Mock-tested only |

## Platform support

| Platform | Result |
| --- | --- |
| macOS arm64 | Local verify, 25-scenario acceptance, package, and retained-state replay passed |
| Linux arm64 | Current source built offline in Linux; complete non-root/read-only OCI runtime suite passed in distroless |
| Linux amd64 | Configured in CI, not dispatched |
| GitHub macOS | Configured in CI, not dispatched |
| Windows | Configured in CI, not dispatched |
| Kubernetes / vendor examples | Documentation-reviewed only; not submitted |

## Verification outcomes

| Command | Outcome |
| --- | --- |
| `cargo xtask verify` | Passed all 12 stages after remediation |
| `cargo xtask acceptance` | Passed all 25 public-CLI scenarios |
| `cargo xtask package` | Passed; macOS arm64 package created |
| `cargo xtask acceptance-container` | Default build failed because the local container CA does not trust Rust/crates.io; failure was not skipped |
| Linux offline build + OCI runtime acceptance | Passed with networking disabled during build and all runtime cases exercised |
| Retained OpenAI database replay under `env -i` | Passed; same output and zero fresh effects/tool calls/provider sessions |

## Residual risks

- The Rust branch and its CI configuration are still local. Linux amd64, hosted macOS, Windows, release packaging, Trivy, SBOM, and secret-scan jobs have no hosted execution record.
- GitHub Action dependencies use movable tags rather than commit SHA pins.
- The repository secret scan is pattern-based and does not replace a dedicated history/binary secret scanner.
- Subprocess output is collected in memory without a byte ceiling; subprocesses are explicitly allowlisted, bounded by time, and run with a cleared environment, but this remains P2 hardening.
- Only OpenAI has retained live provider evidence. Azure OpenAI, Anthropic, Google, MCP, and A2A remain mock-tested.
- SQLite is unencrypted local state and policy allowlists are controls, not an OS sandbox.
- At-most-once external calls can remain uncertain after a dispatch/acknowledgement crash window; this is a documented reconciliation boundary.

Closest human review should focus on `crates/agentctl-runtime/src/lib.rs`, `crates/agentctl-store/src/lib.rs`, `crates/agentctl-providers/src/lib.rs`, `crates/agentctl-protocols/src/lib.rs`, `crates/agentctl-core/src/policy.rs`, `crates/agentctl-cli/src/main.rs`, `xtask/src/acceptance.rs`, `Containerfile`, and `.github/workflows/ci.yml`.
