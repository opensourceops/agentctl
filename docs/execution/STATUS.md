# Execution status

Last updated: 2026-07-22

## Current phase

ready as a `v1alpha1` release candidate

The adversarial audit and final live durable-replay gate passed the defined local, scheduled, and native-arm64 OCI implementation boundary. This is not a stable-v1 recommendation.

## Accepted evidence

- The independently audited Rust implementation passes all 12 `cargo xtask verify` gates (66 tests) and the 25-scenario credential-free public-CLI acceptance suite from a clean copy with Node tools poisoned.
- A packaged GPT-5.6 workflow made one real model-selected read-only tool call and continued through stored-response function output; the final run used two provider requests, 530 input tokens, and 33 output tokens.
- The exact completed live database is retained locally and replays in the native-arm64 image with no credential and `--network none`. Replay has a distinct run/trace ID, identical output, unchanged artifact digest, zero fresh effects/tool calls/provider sessions, and explicit source-effect audit links.
- The deterministic replay regression uses provider and tool executors that panic if called.
- Confirmed effects survive resume; fork is distinct and fresh; timeout/transport uncertainty blocks unsafe repetition.
- Clean copied/source-installed/package layouts, empty-environment cron invocation, concurrency, SIGTERM, approvals, machine output, and recovery paths passed.
- The actual OCI image passed mock-tool, failure-exit, SIGTERM, and offline-replay cases as non-root with a read-only root and mounted durable state/artifacts. Trivy 0.70.0 found no HIGH/CRITICAL findings with or without `--ignore-unfixed`; a CycloneDX SBOM was generated.

## Product boundary

`agentctl` is a schedulable local runtime, not a scheduler or distributed control plane. The workflow API remains alpha and scheduling is sequential. Provider, filesystem, process, and network policy is not an OS sandbox. At-most-once external work can require manual reconciliation. See [Limitations](../LIMITATIONS.md) for the complete release-blocker/hardening/post-v1/non-goal classification.

## External evidence not claimed

The local environment executed macOS arm64 packaging and Linux arm64 OCI tests. The configured GitHub Linux amd64, macOS, Windows, vendor-pipeline, Trivy, and SBOM jobs were not remotely dispatched in this task; GitHub YAML was parsed locally and the remaining examples were documentation-reviewed only. Anthropic, Google, Azure OpenAI, MCP, and A2A remain native mock-tested rather than live-tested.

## Hard blockers

No known P0/P1 implementation or evidence blocker remains for the stated boundary. See [BLOCKERS.md](BLOCKERS.md), [RELEASE_AUDIT.md](RELEASE_AUDIT.md), and [LIVE_OPENAI_REPLAY_EVIDENCE.md](LIVE_OPENAI_REPLAY_EVIDENCE.md).

## Exact commands

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask package
```

The final live journey used the packaged CLI directly to stay within the four-request authorization; normal repository verification remains credential-free.
