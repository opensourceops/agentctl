# Execution status

Last updated: 2026-07-22

## Current phase

Ready for hosted RC validation

The independent release-candidate review found and remediated one P0, five P1s, and scoped journey P2s. The final local hardening adds bounded subprocess capture, durable limit/cancellation regressions, full-SHA hosted workflows, complete-history/tree secret scanning, production/image SBOM gates, and a secure optional build CA path. Local deterministic verification, public CLI acceptance, packaging, current-source OCI acceptance, current Trivy/SBOM validation, and keyless replay evidence pass. The exact recommendation is **Ready for hosted RC validation** because the workflows have not yet run on GitHub.

## Accepted evidence

- The independently audited Rust implementation passes all 12 `cargo xtask verify` gates and the 25-scenario credential-free public-CLI acceptance suite.
- A packaged GPT-5.6 workflow made one real model-selected read-only tool call and continued through stored-response function output; the final run used two provider requests, 530 input tokens, and 33 output tokens.
- The exact completed live database is retained locally and replays in the native-arm64 image with no credential and `--network none`. Replay has a distinct run/trace ID, identical output, unchanged artifact digest, zero fresh effects/tool calls/provider sessions, and explicit source-effect audit links.
- The deterministic replay regression uses provider and tool executors that panic if called.
- Confirmed effects survive resume; fork is distinct and fresh; timeout/transport uncertainty blocks unsafe repetition.
- Clean copied/source-installed/package layouts, empty-environment cron invocation, concurrency, SIGTERM, approvals, machine output, and recovery paths passed.
- A current-source Linux arm64 binary built offline and passed mock-tool, failure-exit, SIGTERM, and offline-replay cases in the production distroless image as non-root with a read-only root and mounted durable state/artifacts.
- The current image built through a secret-mounted CA/tmpfs trust path, passed the full OCI suite, had zero fixed HIGH/CRITICAL findings under checksum-verified Trivy 0.72.0, and produced valid CycloneDX JSON.
- actionlint 1.7.12 accepted every workflow; Gitleaks 8.30.1 found no complete-history or tracked-tree leaks and rejected the generated synthetic credential.

## Product boundary

`agentctl` is a schedulable local runtime, not an external scheduler or
distributed control plane. The workflow API remains alpha. One run may execute
bounded independent tasks concurrently with deterministic plan-order commits;
cross-run and cross-host overlap remains external. Provider, filesystem,
process, and network policy is not an OS sandbox. At-most-once external work
can require explicit reconciliation. See [Limitations](../LIMITATIONS.md) for
the complete release-blocker/hardening/post-v1/non-goal classification.

## External evidence not claimed

The local environment executed macOS arm64 packaging and Linux arm64 OCI runtime/security tests. The configured GitHub Linux x64, macOS arm64, Windows x64, Trivy, Gitleaks, package, and SBOM jobs do not exist on the remote default branch and were not dispatched. No hosted artifact digest or branch-protection result is claimed. Anthropic, Google, Azure OpenAI, MCP, and A2A remain native mock-tested rather than live-tested.

## Release-candidate blockers

No known P0/P1 implementation defect remains for the stated boundary. The remaining gate is hosted execution and artifact evidence for the exact candidate commit. See [HOSTED_CI_PREPARATION.md](HOSTED_CI_PREPARATION.md), [BLOCKERS.md](BLOCKERS.md), and [Release process](../RELEASE_PROCESS.md).

## Exact commands

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask package
```

The final live journey used the packaged CLI directly to stay within the four-request authorization; normal repository verification remains credential-free.
