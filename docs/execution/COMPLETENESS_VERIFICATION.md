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

## OCI completion checkpoint

Date: 2026-07-27, Asia/Kolkata.

The existing Podman machine was recovered without deleting it or changing its
security configuration. Its libkrun and `gvproxy` processes remained available
while the starting terminal stayed open.

| Gate | Result |
| --- | --- |
| `cargo test -p xtask` | passed all 7 tests, including Docker/Podman image-ID normalization and malformed-ID rejection |
| `env -u OPENAI_API_KEY cargo xtask acceptance-container` | passed on Podman 5.8.2, native Linux arm64 |
| Trivy 0.72.0 HIGH/CRITICAL fixed-vulnerability gate | passed with a freshly updated database and zero findings |
| Trivy 0.72.0 CycloneDX generation and `jq` validation | passed; 11 components, 20,821 bytes |
| Image configuration and history marker scan | passed; zero credential, authorization, bearer, or token-pattern lines |

The first current-source run built the image and exposed a product-gate
compatibility defect: Podman 5.8 returned `.Id` as bare 64-hex while the
acceptance parser required Docker's `sha256:` prefix. The parser now validates
both forms, normalizes both to `sha256:<digest>`, and rejects every malformed
form. The cached rerun passed action-level digest-pinned isolation, strict tool
continuation, declared artifact export, durable inspection, parallel ordered
commit, selective repair, network-disabled replay, missing credentials,
invalid input, SIGTERM, non-root identity, read-only root, and mounted state
and artifacts.

The exact image is Linux arm64, `nonroot:nonroot`, version `0.2.0`, and source
`opensourceops/agentctl`. Its local image digest is
`sha256:ddcf174ab2b1ce2481395380d482292a41d79ee5f4620fd52cbd3733e712127c`.
The ignored SBOM at
`.runtime/scan/agentctl-framework-completeness.cdx.json` has SHA-256
`0ee27f16491108f0f018ac2bc7ad201b3f97bdb539cd6b78584264c9eedd67ff`.
No OpenAI request occurred.

## Composite and live checkpoint

Date: 2026-07-27, Asia/Kolkata.

| Gate | Result |
| --- | --- |
| `env -u OPENAI_API_KEY cargo xtask completeness` | passed all three packaged composite groups |
| `env -u OPENAI_API_KEY cargo xtask acceptance-container` | passed the durable composite and compensation/reconciliation additions on native Linux arm64 |
| `cargo xtask examples-verify-live-openai` local phase | passed every public OpenAI example and the 11-request framework composite |
| `cargo xtask examples-verify-live-openai-container` | passed the focused native Linux arm64 live source, repair, and keyless replay continuation |

The deterministic gate verified approval, parallel scheduling, a four-child
matrix, typed agent output, failed-only retry, repair after workspace artifact
deletion, CAS verify/export, keyless replay, sub-workflows, a two-iteration
loop, typed routing, MCP reconnect, compensation, reconciliation,
cancellation, and structured handoffs.

The retained GPT-5.6 matrix used 27 requests, 3,939 input tokens, 560 output
tokens, 20 reasoning tokens, and 8 tool calls. It covered basic and tool
workflows, two agent branches, a two-child agent matrix, structured routing,
an agent loop, an agent sub-workflow, a typed two-role handoff, a deterministic
agent failure and failed-only retry, selective repair, artifact CAS reuse,
keyless replay, real streaming, and Linux arm64 container execution. The
separate one-request resource-budget run denied request two before dispatch.
See
[`LIVE_FRAMEWORK_VERIFICATION.md`](LIVE_FRAMEWORK_VERIFICATION.md)
for run IDs, cumulative request accounting, and fail-closed gate corrections.

Two acceptance defects were corrected before closure. Repair planning now
returns a structured `resolved_input_unavailable` block instead of aborting
when an earlier incompatible reused task makes a downstream template
unresolvable. The live OCI gate now copies the actual public OpenAI repair
workflows rather than the credential-free fixture with unrelated task IDs.

## Evidence rules

- Deterministic gates run without provider credentials.
- Live gates use only `gpt-5.6`, at most 80 Responses API requests for this
  program, and a target aggregate cost below USD 15.
- Live records retain only scenario, model, request/tool counts, token counts,
  run ID, outcome, and recovery/replay reuse status.
- Raw model content, databases, and keys stay in ignored local evidence.
- Hosted jobs are described as executed only when exact-head run records and
  retained artifact digests exist.
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
| Legacy upgrades | all retained schema fixtures, dry-run, rollback, import, boundary, repair/replay tests | migration verification command and the full deterministic gate passed | verified |
| Reconciliation | immutable transition matrix, schema/tool/hook/policy, repair and resume tests | packaged operational composition and native Linux arm64 compensation/reconciliation passed | verified |
| Terminal retry | runtime/store identity, roots, acknowledgements, reconciliation, lineage, source immutability, and replay tests passed | packaged durable pipeline and native Linux arm64 failed-only retry passed | verified |
| Sensitive-state encryption | authenticated context, wrong-key, tamper, inventory, stale-writer trigger, rollback, rotation, checkpoint, and retained-schema tests passed | packaged CLI scenario 31 and the 12-stage verification gate passed | verified |
| Secret references | environment compatibility, file bounds/missing/symlink containment, process allowlist/timeout/output/cancellation, zeroizing values, adapter redaction, and raw-database absence tests passed | packaged CLI scenario 32 and the 12-stage verification gate passed | verified |
| Parallel scheduling | overlap, caps, conflicts, ordered atomic commits, approvals, cancellation, retry, repair, and replay tests passed | packaged durable/native OCI composites and two live agent branches passed | verified |
| Foreach/matrix | compiler bounds/identity tests and runtime partial-failure, child retry, sibling reuse, aggregation, and replay tests passed | packaged durable/native OCI composites and the live two-child agent matrix passed | verified |
| Conditions/routers | compiler typed-case/guard failures and runtime durable condition, route, retry, changed-input repair, and skipped replay tests passed | packaged operational route and live structured-agent route passed | verified |
| Bounded loops | compiler bounds/identity tests and runtime zero/one/max, exhaustion, cancellation, uncertain effect, retry, repair, and replay tests passed | packaged operational loop and live two-iteration agent loop passed | verified |
| Sub-workflows | compiler namespacing/version/cycle/state-isolation tests and runtime typed boundary, retry, repair, and replay tests passed | packaged operational composition, integrity-pinned pack example, and live agent sub-workflow passed | verified |
| Compensation | compiler declarations plus runtime reverse order, approval, partial failure, source and inverse uncertainty, cancellation, retry, reconciliation, automatic trigger, repair/retry invalidation, and replay tests passed | packaged operational composition and native Linux arm64 compensation/reconciliation/replay passed | verified |
| Structured handoffs | compiler rejects hidden teams; typed role and handoff contracts use ordinary task recovery | packaged completeness and live collector/reviewer handoff passed | verified |
| Streaming | provider SSE fragmentation plus runtime bounds, redaction, final validation, and replay tests passed | packaged CLI scenario 40 and live OpenAI streaming marker/replay passed | verified |
| MCP/A2A resilience | 18 protocol tests, 93 runtime tests plus one ignored environment-gated container test, schema 13 migration/encryption/replay tests, and `cargo xtask protocol-resilience` passed | packaged CLI scenario 41 reconnects MCP once and continues one known A2A task through CAS, retry, and replay without resubmission | verified |
| Packs/trust/extensions | 6 resolver/trust tests and 3 focused runtime protocol tests cover graph, sources, locks, Sigstore, trust gating, bounds, cancellation, redaction, and replay | packaged CLI scenario 42 verifies a two-pack lock plus one explicitly authorized process extension invocation and effect-free replay | verified |
| Semantic memory | typed contracts; stable text/vector/hybrid ranking; filters, namespace, expiry, corrupt-dimension, encryption, external-adapter, OpenAI WireMock, credential-preflight, repair, and replay tests passed | packaged CLI scenario 43 covers hybrid retrieval, explicit promotion, CLI put/search/reindex, changed-memory repair, and effect-free replay | verified |
| Network enforcement | scheme/host/port, IPv4/IPv6 classification, every-answer validation, DNS pinning, proxy default deny, redirect refusal, CA success/failure, response bounds, and deterministic credential preflight passed | packaged CLI scenario 44 denies private egress before persistence or I/O | verified |
| Process isolation | DSL validation, plan requirements, command construction, environment, working directory, output/time/cancellation, process-tree cleanup, resource flags, missing-backend tests, and engine image-ID normalization passed | packaged CLI scenario 45 fails a requested unavailable engine before effect dispatch; the real content-addressed container action passed through native Linux arm64 Podman | verified |
| Resource and cost budgets | compiler, store, runtime, CLI, retry/repair/replay, race, and exact-bound tests passed | packaged CLI scenario 46 plus a one-request live GPT-5.6 denial-before-request-2 gate passed | verified |
| Stateless provider continuation | compiler/provider/runtime tests cover negotiation, ordered multiple calls, opaque reasoning, malformed items, encrypted persistence, pause/resume, repair freshness, and replay | packaged GPT-5.6 stateless tool run and keyless zero-effect replay passed | verified |
| Container/cross-platform | native Linux arm64 OCI runtime, signal, durable composite, retry, repair, CAS export, compensation/reconciliation, replay, Trivy 0.72.0, SBOM, and image inspection passed | exact-head hosted Linux x64, macOS arm64, Windows x64, container, security, package, and SBOM gates execute under XPLAT-001 | verified |
| OpenAI live matrix | 27-request retained GPT-5.6 matrix, 3,939 input tokens, 560 output tokens, 20 reasoning tokens, and 8 tool calls | packaged macOS arm64 and native Linux arm64 OCI runs cover basic, tool, parallel, matrix, route, loop, sub-workflow, handoff, retry, repair, CAS, replay, streaming, and budget boundaries | verified |
| Canonical and Pages docs | canonical docs and generated schema passed the full local documentation gate | Pages imported 61 canonical pages; writing, spelling, 70 HTML routes/links, search, and 44 responsive/accessibility browser tests passed | verified |

## Final adversarial review

Status: passed with no unresolved P0, P1, or scoped P2 finding.

The clean pass reviewed security and secret handling, all retained migrations,
effect and reconciliation safety, artifact integrity, concurrent scheduling,
retry/repair/replay lineage, provider continuation, protocol reconnect, pack
trust, selected-field encryption, budgets, container behavior, every public
example, live evidence, hosted workflow coverage, and public claims.

It found zero P0s, three P1s, and five scoped P2s. The P1s were a repair plan
that aborted instead of returning a blocked-input diagnostic, an OCI live gate
that copied the wrong repair fixture, and a live continuation that could check
an existing cost overrun only after dispatch. The scoped P2s were
model/tool-effect miscounting, a two-write evidence completion window, a
missing documented artifact-store command, missing completeness execution in
the hosted matrices, and stale pre-burn-down public status text. All findings
were fixed and covered by focused tests or local validation. The final
credential-free gates, actionlint, deterministic secret scan, native Linux
arm64 OCI acceptance, exact-image Trivy/SBOM checks, and retained GPT-5.6
evidence are green.
