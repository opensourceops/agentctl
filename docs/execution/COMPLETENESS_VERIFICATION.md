# Framework completeness verification

This record accumulates sanitized evidence for the framework-completeness
program. It contains no credentials, raw provider responses, prompt
transcripts, runtime databases, private certificates, or artifact bytes.

## Baseline

Branch point: `af9b4ae`, including independently reviewed selective workflow
repair.

Date: 2026-07-23, Asia/Kolkata.

| Gate | Baseline result |
| --- | --- |
| `cargo xtask verify` | passed all 12 stages |
| `cargo xtask acceptance` | passed 28 scenarios |
| `cargo xtask examples-verify` | passed |
| `cargo xtask docs-verify` | passed all 6 stages |
| `cargo xtask package` | passed for macOS arm64 |
| `cargo xtask secret-scan` | passed |
| `env -u OPENAI_API_KEY cargo xtask acceptance-container` | reached the OCI CLI, then failed because `/artifacts/report.txt` escaped the authorized workspace root |

The installed container runtime is Podman 5.8.2 with a libkrun machine. In this
execution environment, Podman's VM and forwarding processes survive only while
the starting terminal remains open. Keeping that terminal active made the
engine reachable without deleting a machine, changing TLS, or weakening
configuration.

No OpenAI request was made during baseline verification.

## Semantic-memory checkpoint

Date: 2026-07-27, Asia/Kolkata.

| Gate | Result |
| --- | --- |
| `cargo xtask verify` | passed all 12 stages; 14 CLI, 55 core, 21 provider, 80 runtime, 31 store, and 15 protocol tests passed |
| `cargo xtask acceptance` | passed all 43 credential-free packaged CLI scenarios |
| `cargo xtask docs-verify` | passed all 6 stages |
| `cargo xtask examples-verify` | passed |
| `cargo xtask package` | passed for macOS arm64 |
| `cargo xtask secret-scan` | passed |

All deterministic commands ran with `OPENAI_API_KEY` removed. The optional
OpenAI embedding adapter used only WireMock contract tests, so this checkpoint
made zero OpenAI API requests.

## Process-isolation checkpoint

Date: 2026-07-27, Asia/Kolkata.

| Gate | Result |
| --- | --- |
| `cargo check --workspace --all-targets` | passed without warnings |
| `cargo test -p agentctl-core --lib` | passed all 63 tests |
| `cargo test -p agentctl-store --lib` | passed all 35 tests |
| `cargo test -p agentctl-runtime --lib` | passed 90 tests; one live container test ignored by default |
| `cargo test -p agentctl-runtime process::tests::` | passed all 8 process lifecycle, cleanup, and container-contract tests |
| `cargo xtask acceptance` | passed all 46 credential-free packaged CLI scenarios |
| `cargo xtask resource-budget-live-openai` | passed with one GPT-5.6 request, 18 input tokens, and 5 output tokens; second request denied |
| `env -u OPENAI_API_KEY cargo xtask acceptance-container` | blocked before image build because Podman 5.8.2/libkrun `gvproxy` exited and both the configured TCP endpoint and forwarded Unix socket refused connections |

The existing Podman VM was started and then cleanly stopped/restarted once.
The VM booted, but its forwarding process exited immediately on both attempts.
No image or container was started by this checkpoint. The OCI gate now includes
a real digest-pinned action-level container test and must be rerun when the
local engine is healthy.

## Evidence rules

- Deterministic gates run without provider credentials.
- Live gates use only `gpt-5.6`, at most 80 Responses API requests for this
  program, and a target aggregate cost below USD 15.
- Live records retain only scenario, model, request/tool counts, token counts,
  run ID, outcome, and recovery/replay reuse status.
- Raw model content, databases, and keys stay in ignored local evidence.
- Configured hosted jobs are not described as executed.
- Native and emulated container architecture results are labeled explicitly.
- Every verified limitation links to focused tests plus at least one public
  product path.

## Required deterministic gates

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask examples-verify
cargo xtask docs-verify
cargo xtask package
cargo xtask secret-scan
cargo xtask artifact-store-verify
cargo xtask migration-verify
cargo xtask protocol-resilience
cargo xtask completeness
```

## Required opt-in gates

```console
cargo xtask examples-verify-live-openai
cargo xtask resource-budget-live-openai
cargo xtask acceptance-container
```

## Workstream evidence

| Workstream | Focused evidence | Composite evidence | Status |
| --- | --- | --- | --- |
| Artifact CAS | 19 store tests and 38 runtime tests | CLI acceptance and hardened OCI acceptance passed | verified |
| Legacy upgrades | all retained schema fixtures, dry-run, rollback, import, boundary, repair/replay tests | migration verification command added; full composite rerun pending | verified |
| Reconciliation | immutable transition matrix, schema/tool/hook/policy, repair and resume tests | full composite rerun pending | verified |
| Terminal retry | runtime/store identity, roots, acknowledgements, reconciliation, lineage, source immutability, and replay tests passed | packaged CLI scenario 30 and the 12-stage verification gate passed | verified |
| Sensitive-state encryption | authenticated context, wrong-key, tamper, inventory, stale-writer trigger, rollback, rotation, checkpoint, and retained-schema tests passed | packaged CLI scenario 31 and the 12-stage verification gate passed | verified |
| Secret references | environment compatibility, file bounds/missing/symlink containment, process allowlist/timeout/output/cancellation, zeroizing values, adapter redaction, and raw-database absence tests passed | packaged CLI scenario 32 and the 12-stage verification gate passed | verified |
| Parallel scheduling | overlap, caps, conflicts, ordered atomic commits, approvals, cancellation, retry, repair, and replay tests passed | packaged CLI scenario 33 and OCI parallel run/replay passed | deterministic verified; live pending |
| Foreach/matrix | compiler bounds/identity tests and runtime partial-failure, child retry, sibling reuse, aggregation, and replay tests passed | packaged CLI scenario 34 passed | deterministic verified; live pending |
| Conditions/routers | compiler typed-case/guard failures and runtime durable condition, route, retry, changed-input repair, and skipped replay tests passed | packaged CLI scenario 35 passed | deterministic verified; live pending |
| Bounded loops | compiler bounds/identity tests and runtime zero/one/max, exhaustion, cancellation, uncertain effect, retry, repair, and replay tests passed | packaged CLI scenario 36 passed | deterministic verified; live pending |
| Sub-workflows | compiler namespacing/version/cycle/state-isolation tests and runtime typed boundary, retry, repair, and replay tests passed | packaged CLI scenario 37 and integrity-pinned pack example passed | deterministic verified; live pending |
| Compensation | compiler declarations plus runtime reverse order, approval, partial failure, source and inverse uncertainty, cancellation, retry, reconciliation, automatic trigger, repair/retry invalidation, and replay tests passed | packaged CLI scenario 38 and the full local release gate passed | verified |
| Structured handoffs | compiler rejects hidden teams; typed role and handoff contracts use ordinary task recovery | packaged CLI scenario 39 covers tool separation, inspection, retry, repair, and replay | deterministic verified; live pending |
| Streaming | provider SSE fragmentation plus runtime bounds, redaction, final validation, and replay tests passed | packaged CLI scenario 40 covers human, JSONL, final JSON, inspection, and replay | deterministic verified; live pending |
| MCP/A2A resilience | 15 protocol tests, 82 runtime tests plus one ignored environment-gated container test, schema 13 migration/encryption/replay tests, and `cargo xtask protocol-resilience` passed | packaged CLI scenario 41 reconnects MCP once and continues one known A2A task through CAS, retry, and replay without resubmission | verified |
| Packs/trust/extensions | 6 resolver/trust tests and 3 focused runtime protocol tests cover graph, sources, locks, Sigstore, trust gating, bounds, cancellation, redaction, and replay | packaged CLI scenario 42 verifies a two-pack lock plus one explicitly authorized process extension invocation and effect-free replay | verified |
| Semantic memory | typed contracts; stable text/vector/hybrid ranking; filters, namespace, expiry, corrupt-dimension, encryption, external-adapter, OpenAI WireMock, credential-preflight, repair, and replay tests passed | packaged CLI scenario 43 covers hybrid retrieval, explicit promotion, CLI put/search/reindex, changed-memory repair, and effect-free replay | verified |
| Network enforcement | scheme/host/port, IPv4/IPv6 classification, every-answer validation, DNS pinning, proxy default deny, redirect refusal, CA success/failure, response bounds, and deterministic credential preflight passed | packaged CLI scenario 44 denies private egress before persistence or I/O | verified |
| Process isolation | DSL validation, plan requirements, command construction, environment, working directory, output/time/cancellation, process-tree cleanup, resource flags, and missing-backend tests passed | packaged CLI scenario 45 fails a requested unavailable engine before effect dispatch; real container action is wired into the OCI gate but locally blocked by Podman forwarding failure | deterministic verified; OCI pending |
| Resource and cost budgets | pending | pending | open |
| Container/cross-platform | baseline defect recorded | pending | in progress |
| OpenAI live matrix | retained selective-repair evidence only | pending | open |
| Canonical and Pages docs | limitation register created | pending | in progress |

## Final adversarial review

No final review result is recorded yet. Completion requires a clean review pass
over security, migrations, effects, artifacts, concurrency, recovery,
protocols, pack trust, encryption, budgets, containers, examples, and live
evidence, followed by remediation of every P0/P1 and scoped P2 finding.
