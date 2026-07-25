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
| Handoffs/streaming | pending | pending | open |
| MCP/A2A resilience | pending | pending | open |
| Packs/trust/extensions | pending | pending | open |
| Semantic memory | pending | pending | open |
| Network/isolation/budgets | pending | pending | open |
| Container/cross-platform | baseline defect recorded | pending | in progress |
| OpenAI live matrix | retained selective-repair evidence only | pending | open |
| Canonical and Pages docs | limitation register created | pending | in progress |

## Final adversarial review

No final review result is recorded yet. Completion requires a clean review pass
over security, migrations, effects, artifacts, concurrency, recovery,
protocols, pack trust, encryption, budgets, containers, examples, and live
evidence, followed by remediation of every P0/P1 and scoped P2 finding.
