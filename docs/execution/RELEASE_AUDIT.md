# Final adversarial release audit

Audit date: 2026-07-22 (Asia/Kolkata)

Historical recommendation: **Ready as a `v1alpha1` release candidate**. This conclusion is superseded by the later [independent RC review](INDEPENDENT_RC_REVIEW.md), which found additional P0/P1 defects and changed current status to **Ready for internal review** pending hosted CI and current image-build evidence. The retained live-run facts below remain historical evidence.

## Final live durable-replay gate

The previously missing proof is now complete. A packaged macOS arm64 CLI executed the canonical GPT-5.6 YAML workflow with a real model-selected read-only tool call and stored-response continuation. The exact completed SQLite database was retained locally, scanned, copied byte-for-byte into the updated production image's state volume, and replayed as non-root with no credential and `--network none`.

The final source run used two provider requests and one tool call (530 input and 33 output tokens). Replay returned the same declared output and unchanged artifact digest while public inspection reported zero fresh effects, tool calls, or provider sessions. A new `replay.effects_reused` audit event links the replay trace to the original run's four effect IDs and tool-call record. The deterministic regression uses provider/tool executors that panic if replay invokes them. Full sanitized evidence is in [LIVE_OPENAI_REPLAY_EVIDENCE.md](LIVE_OPENAI_REPLAY_EVIDENCE.md); exact databases and machine output remain ignored locally.

Two bounded live executions were required: the first revealed that the canonical example's redundant credential-environment reference was serialized into durable workflow state. No key value was present. The reference was removed in favor of the existing provider default, and the final retained database has zero credential-name, authorization, exact-key, or key-fragment matches. Task total was four OpenAI requests, within the authorized maximum.

## Scope and tree identity

The audit began on branch `main` at base commit `be9d0aee0b71e31aeaa386429bb8d7907e01404b` (`push`). The initial working tree contained the intended uncommitted TypeScript-to-Rust migration: 107 tracked paths, 115 intended untracked source paths, and local ignored build/runtime directories. The tracked-diff patch ID was `1e8c3fcff65f7edd8624d6d72f1d379e066941bb`. This base commit plus the initial status inventory is the reviewed pre-commit tree identity.

Initial local-only data included `target` (about 5.5 GiB), `.runtime` (about 1.2 GiB), `node_modules` (about 109 MiB), and `dist` (about 16 MiB). They were ignored and excluded from clean-room source. A tracked legacy runtime database, `examples/memory-flow/state/long-term.db`, was removed; it remains recoverable from Git history. An accidental generated Node-version-only artifact change was restored.

The ledger was changed to `release audit in progress` before verification. No release-candidate status from the preceding run was assumed.

## Initial claims reviewed

| Claim from the preceding run | Audit conclusion |
| --- | --- |
| Rust-only local build, install, deterministic examples, and scheduled execution pass | Confirmed independently from a clean copy with Node/npm/npx/tsc poisoned to fail. |
| Credential-free public-CLI acceptance passes | Confirmed; 25 scenarios pass. |
| Native arm64 OCI execution passes | Confirmed and strengthened with failure exits, SIGTERM, durable inspect, and network-disabled replay. |
| OpenAI GPT-5.6 tool workflow passed live | Prior ledger and bounded usage metadata reviewed; not called again. The implementation path remains mock-tested. |
| Live OpenAI durable state replayed without credentials | Corrected during the original audit because its prior databases were unavailable. The final closure gate above now supplies the missing exact live-state proof. |
| Ambiguous effects safely block resume | Partially false before fixes: several paths remained `started` or were marked `failed`; one subprocess timeout returned before uncertainty recording. Fixed and regression-tested. |
| JSON mode is always parseable | False for Clap parse failures before fixes. Unknown command, missing argument, and invalid value now return the versioned JSON error envelope. |
| Supply-chain verification cannot be silently skipped | False before fixes: `cargo-deny` could be absent while `verify` still succeeded. It is now a required gate and CI installs it. |
| No fixable HIGH/CRITICAL image vulnerabilities | Reworded and strengthened: final Trivy 0.70.0 scans found no HIGH/CRITICAL findings with or without `--ignore-unfixed`. |
| External CI/CD examples were validated | Corrected to documentation-reviewed/YAML-parsed where applicable; none was externally dispatched. |

## Clean-room environment

The final committed-tree source copy was `/private/tmp/agentctl-final-committed.ARlcZj/source`. It was created with `git archive HEAD`, so it contained only committed release source and no `.git`, Rust build output, runtime state, generated test artifacts, credentials, editor state, or existing container state. An earlier pre-commit clean copy used `rsync` with the same source exclusions so intended untracked migration files were audited before commit.

`OPENAI_API_KEY` was removed from every clean-room command. A directory placed first on `PATH` contained `node`, `npm`, `npx`, and `tsc` symlinked to `/usr/bin/false`; all release commands still passed. This proves the production build, installation, tests, examples, package, image build, and workflow execution do not invoke Node or the archived TypeScript runtime.

Environment:

- host: macOS arm64;
- `rustc`/Cargo 1.88.0, host `aarch64-apple-darwin`;
- Podman 5.8.2, native Linux arm64 VM;
- `cargo-deny` available and mandatory;
- Trivy container image 0.70.0 with a local vulnerability database;
- `cargo-llvm-cov`, `syft`, `actionlint`, `kubectl`, `kubeconform`, `hadolint`, and `shellcheck` unavailable; no new host tooling was installed.

Two early clean-room attempts failed as intended: the first caught unformatted new tests, and the second caught a warnings-denied Clippy `needless_borrow`. The current-tree gate then exposed a real subprocess-timeout path that returned before marking uncertainty. A later `git archive HEAD` gate caught an incorrect reusable-pack digest that local pre-commit state had masked. All findings were corrected before the successful committed-tree run. The gates propagated nonzero exit status; they did not mask failure.

The local Podman VM sits behind an enterprise TLS-interception root trusted by macOS but not by the stock Rust builder image. For a no-cache container rebuild, the already-trusted public root certificate was mounted read-only over the builder's CA bundle. It was never added to source, copied into an image layer, or retained after the build. The unmodified `cargo xtask acceptance-container` command then rebuilt the content-addressed image and executed the complete OCI suite.

## Exact release commands and results

The following commands ran from the final clean copy with `OPENAI_API_KEY` absent and the no-Node `PATH` prefix:

| Command | Exit | Safe result |
| --- | ---: | --- |
| `cargo xtask verify` | 0 | All 12 gates passed: rustfmt, warnings-denied Clippy, workspace build, 66 tests, fuzz-target checks, doc tests/docs, generated schema/CLI consistency, examples/negative contracts, source/license/advisory checks, secret scan, source installation, and Rust-only boundary. |
| `cargo xtask acceptance` | 0 | All 25 public-CLI scenarios passed, including approval/resume, replay/fork, uncertainty, JSON parse errors, cron-like empty environment, concurrency, and SIGTERM. |
| `cargo xtask acceptance-container` | 0 | Native arm64 OCI success/artifact/inspect, offline replay, missing secret, invalid workflow, SIGTERM, non-root, read-only root, and mount cases passed. |
| `cargo xtask package` | 0 | Produced the optimized macOS arm64 package and four shell completions. |
| `shasum -a 256 -c SHA256SUMS` | 0 | `agentctl: OK`. |
| `git diff --check` | 0 | No whitespace errors. |

The final durable-replay closure used the packaged production CLI directly rather than the four-request host-plus-container live harness. It made four total OpenAI requests across two bounded executions and then performed the exact final replay without credentials or network. See [LIVE_OPENAI_REPLAY_EVIDENCE.md](LIVE_OPENAI_REPLAY_EVIDENCE.md).

GitHub workflow YAML was parsed locally with Ruby's YAML parser. GitLab, Jenkins, Harness, Docker, Kubernetes Job, and Kubernetes CronJob examples were documentation-reviewed but not dispatched or vendor-validated.

## Findings and remediation

No P0 finding was identified.

### P1 — ambiguous effects were not explicitly uncertain

- Symptom: provider timeout/transport could remain `started`; subprocess timeout/cancellation/I/O and MCP/A2A errors could be recorded as `failed`; tool timeout/cancellation could mark the tool call failed or leave the effect started. A `?` in subprocess timeout selection also bypassed the later uncertainty handler.
- Root cause: the store lacked explicit uncertainty transitions and dispatch layers collapsed transport ambiguity into ordinary execution errors.
- Affected journeys: resume after interruption, scheduled cancellation, remote/provider/tool/process effects.
- Change: added durable effect/tool-call uncertainty transitions; classified provider, process, tool, MCP, and A2A ambiguous outcomes conservatively; enriched uncertainty errors with run/trace/effect correlation.
- Regression: provider timeout acceptance; tool timeout/cancellation runtime test; subprocess timeout runtime test; protocol ambiguity classification test; resume assertion.
- Result: uncertain effects are visible through `inspect`, are never automatically repeated, and resume exits `3` with run/trace correlation.

### P1 — unsupported OpenAI stateless tool continuation

- Symptom: a tool-using OpenAI/Azure agent could set `store: false`, while continuation still used `previous_response_id` and did not replay stateless response/reasoning/function items.
- Root cause: provider-option validation did not account for runtime continuation semantics.
- Affected journey: multi-turn OpenAI/Azure function calling.
- Original audit change: compilation rejected `store: false` for tool-using
  OpenAI/Azure agents rather than pretending it was supported.
- Framework-completeness follow-up: the adapter now requests encrypted
  reasoning content, persists provider-neutral ordered continuation items, and
  replays them with correlated function outputs. The compiler restriction was
  removed after deterministic pause/resume/replay coverage was added.
- Regression: compiler unit test and credential-free acceptance negative contract.
- Result: no unsupported continuation is silently emitted. This matches the official [function-calling continuation contract](https://developers.openai.com/api/docs/guides/function-calling) and [reasoning-item context guidance](https://developers.openai.com/api/docs/guides/reasoning#keeping-reasoning-items-in-context).

### P1 — JSON parse failures bypassed the machine contract

- Symptom: unknown commands, missing arguments, and invalid values used Clap's human error even when `--output json` was requested.
- Root cause: `Cli::parse()` exited before application rendering.
- Affected journey: CI/scheduled callers consuming the versioned machine interface.
- Change: pre-detect JSON mode, use fallible parsing, preserve help/version behavior, and emit a versioned exit-2 JSON error.
- Regression: CLI unit tests and three public-binary acceptance cases.
- Result: each tested parse failure is valid `agentctl.dev/cli/v1` JSON on stderr.

### P1 — committed reusable-pack example failed integrity verification

- Symptom: `cargo xtask verify` from `git archive HEAD` rejected `examples/v1/reusable-pack.yaml` because its pinned digest did not match the committed pack manifest.
- Root cause: the reference retained a digest from earlier local content, and the pre-commit verification environment did not expose the committed-tree mismatch.
- Affected journey: source checkout verification and the documented reusable-pack example.
- Change: recomputed and pinned the SHA-256 digest of the committed `example.pack.yaml` content.
- Regression: the canonical example/negative-contract gate checks the reference by running the public `agentctl check` command.
- Result: the committed-tree 12-gate verification and reusable-pack execution pass.

### P2 — replay could create partial state for a nonterminal source

- Symptom: a replay row could be created before source task terminality was validated.
- Root cause: validation occurred inside the copy loop.
- Change: validate and map source run/tasks before replay creation.
- Regression: paused-source replay test asserts rejection and unchanged run count.
- Result: invalid replay attempts leave no partial replay run.

### P2 — release gates and container acceptance were incomplete

- Symptom: `cargo-deny` could be skipped; OCI acceptance covered only a successful mock run; CI examples omitted bounded timeouts and recoverable approval-state collection.
- Root cause: optimistic prerequisite handling and narrow happy-path acceptance.
- Change: require `cargo-deny`, install it in verification/release workflows, and add OCI missing-secret, invalid-input, artifact/inspect, `--network none` replay, and SIGTERM cases. CI/CD examples now use bounded timeouts and document protected state retention/recovery.
- Regression: the canonical gates themselves.
- Result: missing prerequisites/failures are nonzero; container exit codes and PID1 signal handling are exercised.

### P2 — repository hygiene and claims

- Removed the tracked SQLite runtime database and retained database/build/runtime ignores.
- Removed dead `xtask` code and its `allow(dead_code)`.
- Replaced “safe”/production-readiness wording with policy-constrained, production-oriented `v1alpha1` language.
- Corrected secret-loading timing, provider maturity, live-evidence, architecture, external-CI, and scan claims.

## Durable execution review

| Failure window | Durable behavior |
| --- | --- |
| Before request record | No external dispatch occurs. |
| After request record, before dispatch | Status remains `requested`; resume may dispatch it after policy/approval checks. |
| During dispatch | Status is `started`; timeout, cancellation, transport loss, or ambiguous I/O changes it to `uncertain`. |
| External commit before local acknowledgement | Local state remains `uncertain`; automatic resume is refused. |
| Confirmed result before task-state commit | The confirmed effect result is reused; it is not dispatched again. |
| Task-state transition before checkpoint | Task transition and checkpoint write share one SQLite transaction, preventing that partial local state. |

Effect identity includes run, task, task attempt, ordinal, operation, and input digest. Attempts and trace IDs are inspectable. `fork` is the explicit operation that permits fresh effects. No exactly-once external-execution claim is made.

## Offline replay proof

The deterministic runtime regression starts a tool-calling provider workflow, replays it against provider/tool executors that panic if invoked, asserts identical structured output, proves zero replay effects/tool calls, and verifies the replay audit's exact source effect/tool-call references.

The public OCI journey then:

1. ran the mock tool workflow and retained `/state/runtime.db`;
2. replayed with no credential forwarding and `--network none`;
3. compared the complete structured `/data/output` value;
4. asserted a distinct replay run ID;
5. inspected the replay and found zero effects and zero tool calls.

The same public OCI journey was then repeated with the exact final live OpenAI database. Credential-free auth inspection reported the OpenAI credential absent; the image received no credential or workspace mount and ran with `--network none`. Replay succeeded with a distinct run/trace ID, identical declared output, an unchanged artifact digest, zero fresh effects/tool calls/provider sessions, and explicit source-effect provenance in audit output.

## Provider and protocol support

No adapter except OpenAI is represented as live-tested, and no external provider conformance suite was run.

| Kind | Implementation | Audit validation | Release wording |
| --- | --- | --- | --- |
| Fake | in-process text/tool/usage/continuation | deterministic unit/runtime/public acceptance | Deterministically tested |
| OpenAI Responses | native auth/request/response, strict tools, multiple call IDs, continuation, usage, reasoning/cache options | mock-protocol mapping plus final bounded GPT-5.6 live tool run and exact offline durable replay | Live-tested and mock-protocol tested |
| Azure OpenAI Responses | native Azure auth/path plus OpenAI mapping | focused mock request/auth/response test | Mock-mapping tested; not live-tested |
| Anthropic Messages | native content/tool/usage mapping | focused mock native tool test | Mock-mapping tested; not live-tested |
| Google Gemini | native content/function/usage mapping | focused mock native response test | Mock-mapping tested; not live-tested |
| MCP 2025-11-25 | native initialization/session/list/call/SSE/cancel/timeout | local mock protocol server tests | Mock-protocol tested; not live-tested |
| A2A 1.0 | native discovery/send/poll/stream/cancel | local mock peer tests | Mock-protocol tested; not live-tested |

## Platform and delivery support

| Platform | Validation |
| --- | --- |
| macOS arm64 host | Native build, 66 tests, public acceptance, installation, package, checksum, packaged GPT-5.6 tool workflow: executed |
| Linux arm64 OCI | Native Podman build/run, non-root/read-only, signals, failures, offline replay, scan/SBOM: executed |
| Linux amd64 OCI | CI-configured only. A local `--platform linux/amd64` build was attempted because Podman advertised emulation, but emulated `rustc` terminated with SIGSEGV; the emulator was not reliable, so no local build/run claim is made. |
| macOS x86_64 | Not tested locally; CI-configured through hosted macOS only when dispatched. |
| Windows x86_64 | Not tested locally; CI-configured only when dispatched. |
| GitHub Actions | Workflow YAML parsed locally; not dispatched. |
| GitLab CI, Jenkins, Harness | Documentation-reviewed examples; not externally dispatched. |
| Kubernetes Job/CronJob | Documentation-reviewed manifest; not submitted to a cluster. |

## Container and security evidence

- Final image: Linux arm64, version label `0.2.0`, user `nonroot:nonroot`, entrypoint `/usr/local/bin/agentctl`.
- A no-cache builder run used a temporary read-only enterprise CA mount solely for dependency download in this network; the certificate is absent from source, build layers, runtime filesystem, image history, labels, and environment defaults.
- Environment defaults contain only PATH and CA-certificate location; no provider credential defaults.
- Image history contains the runtime base, labels, and copied Rust binary; no credential-bearing command.
- Exported root filesystem contains no Node/npm/npx, Rust compiler/Cargo, TypeScript source, workflow, fixture, or build tree. CA certificates are present through the distroless base.
- The image runs with `--read-only`, UID/GID 65532, and only `/state` and `/artifacts` writable.
- Trivy 0.70.0 reported zero HIGH/CRITICAL findings both with and without `--ignore-unfixed`. A 20 KiB CycloneDX JSON SBOM was generated at ignored local evidence path `.runtime/scan/agentctl-final.cdx.json`.
- `cargo deny check`: advisories, bans, licenses, and sources passed. Duplicate dependency versions are warnings, not denied findings.
- Repository and retained-evidence scans found no committed token/private key or exact/fragment key match. The ignored final database contains no provider credential name, authorization header, bearer marker, or environment dump. The configured key value was never printed, passed as an argument, persisted, or forwarded into replay/container state.
- Production Rust contains no `unsafe`, production `panic!`, ignored test, `allow(dead_code)`, or `allow(unused)`. `expect`/`panic!` occurrences are test assertions. `allow(clippy::too_many_arguments)` is limited to explicit effect/transition/store data-flow signatures where named parameters preserve audit meaning. `serde_json::Value` serialization uses an infallible-in-practice fallback for digest construction; malformed external JSON is parsed before reaching that value type.
- Filesystem/process/network controls are policy checks, not an OS sandbox. Untrusted workflows require a restricted OS/container identity and egress controls.

## Critical-path test map

| Guarantee | Evidence |
| --- | --- |
| Parser/schema/compiler/templates | strict/unknown-field, source diagnostic, cycle, deterministic order, property, capability, stateless-tool compilation tests |
| State/persistence/migrations/corruption | state transition, transactional checkpoint, schema upgrade/future version, corruption, lock wait, GC tests |
| Effects/approval/resume/fork | store/runtime tests plus public scenarios 9–16 |
| Replay no dispatch | panic-on-call provider+tool regression and exact live-state OCI public replay inspection |
| Provider/tool continuation | native mapping mocks, call-ID test, schema failures, fake tool acceptance, final live OpenAI continuation |
| Policy/path/redaction | traversal, symlink, host allowlist, secret redaction, invalid UTF-8/read-only artifact tests |
| Cancellation/uncertainty | provider/tool/process/protocol tests and host/container SIGTERM acceptance |
| CLI machine contract | parse errors, validation/auth/policy/run/cancel outputs, run/trace correlation |
| Cron/container | empty-environment non-TTY acceptance and native arm64 OCI acceptance |

Coverage percentage was not invented; `cargo-llvm-cov` was unavailable. Timing-sensitive tool/process/signal tests were run through both the focused suite and repeated canonical acceptance during the audit without a flaky failure.

## Deferred items and residual risks

- Dispatch the configured Linux amd64, macOS, Windows, scan/SBOM, and external pipeline gates; until then they remain CI-configured or documentation-reviewed only.
- Expand Azure/Anthropic/Google adapter negative/error/cancellation/tool-continuation coverage before raising their maturity beyond focused mock mapping.
- Single-host SQLite, local bounded parallel scheduling, explicit uncertain-effect reconciliation, alpha schema evolution, and policy-not-sandbox limitations remain intentional.
- Container bind mounts require deliberate UID/GID 65532 provisioning and protected collection of state, which may contain prompts and outputs.
- Formal external MCP/A2A/provider conformance suites and long-horizon upgrade fixtures are deferred.

Files requiring closest human review are `crates/agentctl-runtime/src/lib.rs` (effect windows and replay), `crates/agentctl-store/src/lib.rs` (uncertainty persistence), `crates/agentctl-providers/src/lib.rs` (OpenAI continuation mapping), `crates/agentctl-protocols/src/lib.rs` (ambiguity classification), `crates/agentctl-cli/src/main.rs` (machine errors/correlation), `xtask/src/acceptance.rs` (release claims), `Containerfile`, and `docs/CONTAINER.md`.

## Final gate decision

There is no known P0/P1 implementation defect in the defined local, externally scheduled, or generic OCI boundary after remediation. Clean-room deterministic and OCI evidence is green, and the exact retained live OpenAI state now passes independent credential-free, network-disabled replay. The honest recommendation is **Ready as a `v1alpha1` release candidate**, not stable v1.0.
