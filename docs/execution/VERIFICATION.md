# Verification record

Date: 2026-07-22, Asia/Kolkata. Secret values were never printed, passed as arguments, placed in YAML, or included in retained evidence.

The independent final audit and the completed exact live-state replay are authoritative in [RELEASE_AUDIT.md](RELEASE_AUDIT.md) and [LIVE_OPENAI_REPLAY_EVIDENCE.md](LIVE_OPENAI_REPLAY_EVIDENCE.md).

## Independent audit corrections

The reopened audit found that the earlier provider-only smoke did not substantiate runtime production readiness. It also found concrete implementation gaps: packaged YAML tools were not registered; declared outputs could not read workflow inputs; traces and provider/tool continuation evidence were not durable/publicly inspectable; non-interactive approvals did not durably pause; SIGTERM and in-flight provider cancellation were misclassified; resume/fork lost the original workspace; missing credentials created partial database state; provider options could be ignored; ambiguous transport failures could be retried; function-call IDs were treated as globally unique; and the repository had no user-journey, cron, or OCI acceptance layer.

All release-blocking gaps above were fixed and covered by focused regression or public-CLI acceptance scenarios. Unsupported OpenAI streaming and programmatic tool calling now fail compilation rather than implying support.

## Repository-owned gates

| Command | Result |
| --- | --- |
| `cargo xtask verify` | passed all 12 gates; 66 unit/integration/compatibility tests, doc tests, six fuzz-target builds, denied-warning clippy, generated artifacts, examples, source install, supply-chain/secret/Rust-only boundaries |
| `cargo xtask acceptance` | passed 25 credential-free public-binary scenarios covering the required deterministic/mock/tool/schema/policy/approval/resume/replay/fork/timeout/retry/auth/output/input/artifact/concurrency/SIGTERM/package-style/cron/quickstart journeys |
| `cargo xtask acceptance-container` | passed on Linux arm64 through Podman: non-root UID/GID, read-only root, mounted config/workspace/state/artifacts, strict tool continuation, parseable JSON, public inspect, expected artifact |
| Manual final live/replay gate | packaged macOS arm64 GPT-5.6 tool workflow passed; its exact retained state replayed in the production Linux arm64 image with no credential, `--network none`, identical output/artifact digest, and zero fresh effects/tool calls |
| `cargo xtask package` | passed; optimized binary, Bash/Zsh/Fish/PowerShell completions, README, license, and SHA-256 manifest at `dist/agentctl-0.2.0-aarch64-apple-darwin` |

The canonical `verify`, credential-free acceptance, container acceptance, and packaging gates use no provider credentials. The final live gate was separately bounded and authorized.

## Live OpenAI evidence

Scenario: `examples/openai-live/workflow.yaml`, model `gpt-5.6`, Responses API, low reasoning, stored response, current-turn reasoning context, implicit 30-minute cache mode, parallel tool calls disabled.

The public path was YAML parse/schema → compiler/plan → capability and policy checks → SQLite run/effect creation → Responses API → strict `read_fixture` function call → tool input validation → workspace policy → real read → output validation → `previous_response_id` continuation → exact final token assertion → atomic report write → checkpoints/audit/traces → CLI result/inspect.

The final retained run used two provider requests, one actual model-selected tool call, 530 input tokens, and 33 output tokens. Its exact database replayed in the image with no credential or network, and public inspection found zero replay effects, tool calls, and provider sessions. Two bounded live executions were needed because the first exposed a persisted credential-environment reference in the canonical YAML; after removing that redundant reference, the compliant final run used the remaining two requests. Task total was four requests, 1,060 input tokens, and 66 output tokens. Provider billing metadata was not returned, so cost was not fabricated.

Official feature/pricing references used for the audit: [GPT-5.6 model catalog](https://developers.openai.com/api/docs/models), [model guidance](https://developers.openai.com/api/docs/guides/model-guidance?model=gpt-5.6), [function calling](https://developers.openai.com/api/docs/guides/function-calling), [reasoning](https://developers.openai.com/api/docs/guides/reasoning), and [prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching).

## Operational and supply-chain evidence

- A copied binary ran help, version, schema, provider diagnostics, completion generation, and the canonical quickstart outside the source tree. `cargo install --path` also passed in a clean temporary root.
- An empty-environment, non-TTY cron-equivalent run passed with stable JSON and explicit paths. Approval pause, overall timeout, SIGTERM exit `130`, concurrent SQLite use, and recovery paths passed.
- Mock tests cover redacted non-retryable authentication errors, explicit 429 retryability, malformed success responses, provider cancellation, tool timeout/cancellation, invalid UTF-8, the 1 MiB workspace-read bound, read-only artifact failure, traversal/symlink escape, database lock/corruption, and protocol malformed/version/origin/timeout cases.
- The OCI image inspection reported `linux arm64`, `nonroot:nonroot`, and version label `0.2.0`. The acceptance invocation used `--read-only` plus only mounted writable state/artifact paths.
- The preceding Trivy run used `--ignore-unfixed`; the final audit repeated Trivy 0.70.0 both with and without that filter and found no HIGH/CRITICAL findings. A CycloneDX JSON SBOM was generated under the ignored `.runtime/scan` verification area.
- GitHub Actions, GitLab CI, Jenkins, Harness CI, Docker, and Kubernetes examples are syntax/documentation-validated only; they were not dispatched to external vendor platforms. Ubuntu CI is configured to execute the Linux amd64 container, scan, and SBOM gates.

`cargo deny check` passed advisories, bans, licenses, and sources. Duplicate-version reports remain reviewed non-blocking warnings.
