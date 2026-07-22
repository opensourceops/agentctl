# Execution status

Last updated: 2026-07-22

## Current phase

ready for internal review

The independent release-candidate review found and remediated one P0, five P1s, and scoped journey P2s. Local deterministic verification, public CLI acceptance, packaging, current-source Linux arm64 compilation, OCI runtime cases, and keyless replay of retained live state pass. The recommendation is not yet release-candidate status because the Rust CI/release workflows have never run on GitHub and the default current-source image build was blocked by this host's container certificate trust.

## Accepted evidence

- The independently audited Rust implementation passes all 12 `cargo xtask verify` gates and the 25-scenario credential-free public-CLI acceptance suite.
- A packaged GPT-5.6 workflow made one real model-selected read-only tool call and continued through stored-response function output; the final run used two provider requests, 530 input tokens, and 33 output tokens.
- The exact completed live database is retained locally and replays in the native-arm64 image with no credential and `--network none`. Replay has a distinct run/trace ID, identical output, unchanged artifact digest, zero fresh effects/tool calls/provider sessions, and explicit source-effect audit links.
- The deterministic replay regression uses provider and tool executors that panic if called.
- Confirmed effects survive resume; fork is distinct and fresh; timeout/transport uncertainty blocks unsafe repetition.
- Clean copied/source-installed/package layouts, empty-environment cron invocation, concurrency, SIGTERM, approvals, machine output, and recovery paths passed.
- A current-source Linux arm64 binary built offline and passed mock-tool, failure-exit, SIGTERM, and offline-replay cases in the production distroless image as non-root with a read-only root and mounted durable state/artifacts.

## Product boundary

`agentctl` is a schedulable local runtime, not a scheduler or distributed control plane. The workflow API remains alpha and scheduling is sequential. Provider, filesystem, process, and network policy is not an OS sandbox. At-most-once external work can require manual reconciliation. See [Limitations](../LIMITATIONS.md) for the complete release-blocker/hardening/post-v1/non-goal classification.

## External evidence not claimed

The local environment executed macOS arm64 packaging and Linux arm64 OCI runtime tests. The committed default OCI build did not complete because the container trust store rejected Rust/crates.io certificates. The configured GitHub Linux amd64, macOS, Windows, Trivy, and SBOM jobs do not exist on the remote default branch and were not dispatched. Anthropic, Google, Azure OpenAI, MCP, and A2A remain native mock-tested rather than live-tested.

## Release-candidate blockers

No known P0/P1 implementation defect remains for the stated boundary. Hosted cross-platform CI and a green committed image-build/scan/SBOM record are still missing. See [INDEPENDENT_RC_REVIEW.md](INDEPENDENT_RC_REVIEW.md), [BLOCKERS.md](BLOCKERS.md), and [LIVE_OPENAI_REPLAY_EVIDENCE.md](LIVE_OPENAI_REPLAY_EVIDENCE.md).

## Exact commands

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask package
```

The final live journey used the packaged CLI directly to stay within the four-request authorization; normal repository verification remains credential-free.
