# Execution status

Last updated: 2026-07-27

## Current phase

Framework completeness locally verified

The limitation burn-down closed every core framework item through
implementation, redesign, or removal from the supported surface. Local
deterministic verification, all public examples, packaging, native Linux arm64
OCI acceptance, current Trivy/SBOM validation, bounded GPT-5.6 verification,
and keyless replay pass. Exact-candidate hosted platform execution remains
externally blocked by this task's no-push/no-dispatch constraint.

## Accepted evidence

- The independently reviewed Rust implementation passes all 12 `cargo xtask
  verify` gates, the 46-scenario credential-free public CLI suite, and all
  three packaged framework-completeness composites.
- The retained GPT-5.6 matrix used 27 requests, 3,939 input tokens, 560 output
  tokens, 20 reasoning tokens, and 8 tool calls across basic agents, tools,
  parallel/matrix/route/loop/sub-workflow/handoff composition, retry,
  selective repair, CAS reuse, replay, streaming, budgets, and OCI execution.
- The completed live runs replay without credentials. Replay has distinct
  run/trace IDs, identical outputs and artifacts, zero fresh effects/tool
  calls/provider sessions, and explicit source lineage.
- The deterministic replay regression uses provider and tool executors that panic if called.
- Confirmed effects survive resume; fork is distinct and fresh; timeout/transport uncertainty blocks unsafe repetition.
- Clean copied/source-installed/package layouts, empty-environment cron invocation, concurrency, SIGTERM, approvals, machine output, and recovery paths passed.
- The native Linux arm64 production image passed mock-tool, approval, durable
  parallel/matrix recovery, retry, repair, CAS export, compensation,
  reconciliation, failure-exit, SIGTERM, and offline-replay cases as non-root
  with a read-only root and mounted durable state/artifacts.
- Image
  `sha256:ddcf174ab2b1ce2481395380d482292a41d79ee5f4620fd52cbd3733e712127c`
  had zero fixed HIGH/CRITICAL findings under Trivy 0.72.0 and produced a valid
  CycloneDX SBOM.
- actionlint 1.7.12 accepted every workflow; Gitleaks 8.30.1 found no complete-history or tracked-tree leaks and rejected the generated synthetic credential.

## Product boundary

`agentctl` is a schedulable local runtime, not an external scheduler or
distributed control plane. The workflow document API is versioned
`v1alpha1`. One run may execute
bounded independent tasks concurrently with deterministic plan-order commits;
cross-run and cross-host overlap remains external. Provider, filesystem,
process, and network policy is not an OS sandbox. At-most-once external work
can require explicit reconciliation. See [Limitations](../LIMITATIONS.md) for
the complete supported-boundary and non-goal classification.

## External evidence not claimed

The local environment executed macOS arm64 packaging and native Linux arm64
OCI runtime/security tests. The exact framework-completeness commit was not
pushed or dispatched on GitHub Linux x64, hosted macOS arm64, or Windows x64.
No exact-candidate hosted artifact digest or branch-protection result is
claimed. Anthropic, Google, Azure OpenAI, MCP, and A2A remain native
mock-tested rather than live-tested.

## Release-candidate blockers

No known P0/P1 implementation defect remains for the stated boundary. Hosted
execution and artifact evidence for the exact candidate commit is the sole
external evidence blocker. See
[HOSTED_CI_PREPARATION.md](HOSTED_CI_PREPARATION.md),
[BLOCKERS.md](BLOCKERS.md), and
[Release process](../RELEASE_PROCESS.md).

## Exact commands

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask examples-verify
cargo xtask completeness
cargo xtask acceptance-container
cargo xtask package
cargo xtask secret-scan
```

The final live journeys used the packaged CLI and bounded native Linux arm64
container continuation. Normal repository verification remains
credential-free.
