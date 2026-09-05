# Autonomous launch-readiness execution ledger

Last updated: 2026-09-06

## Prior environment (historical checkpoint)

- Framework checkout: branch `work`, starting SHA `ec7e220820ed169c005aff5344d81fe4d292cdb6`.
- Documentation-site checkout: unavailable; `/workspace` contains only `agentctl`.
- Runtime OpenAI credential: unavailable (`OPENAI_API_KEY` was tested for presence only).
- GitHub CLI is installed, but no authenticated `GH_TOKEN` is present and this checkout has no configured remote.
- Docker and Podman executables are unavailable.

This historical checkpoint records the earlier environment; the continuation and latest checkpoint below supersede its unresolved environment assumptions. This ledger records only evidence produced for the current task. Historical release evidence remains historical and is not silently promoted to evidence for this checkout.

## Decisions

1. Preserve the existing exclusive `instructions`/`instructionsFile` contract. The runtime already policy-resolves instruction paths, bounds reads at 1 MiB, records the content in the durable effect ledger, fingerprints recorded content for recovery, and uses recorded successful reads when planning reuse.
2. Keep typed inputs and variables as separate namespaces. The current compatible variable order is agent inline defaults, then task inline overrides; explicit invocation flags modify `inputs`, not `vars`.
3. Do not add a partial variable-file syntax. Ordered workflow/agent/task files require origin-aware pack and sub-workflow loading, durable resolved snapshots, source diagnostics, and recovery hashing as one coherent increment. Implementing only the parser/schema would create unsafe or misleading behavior.
4. Add the highest-value bounded adjacent improvement first: `agentctl doctor FILE` performs compilation and reports missing provider credentials and required container-engine executables without resolving secret values or dispatching effects.

## Current feature-to-evidence matrix

| Contract | Primary implementation | Positive evidence | Negative evidence | Current level |
| --- | --- | --- | --- | --- |
| Parse/schema/compiled graph | `agentctl-core` DSL/compiler | `check`, `plan`, core tests | malformed and capability fixtures | automated |
| Explicit policy/approval | core policy, runtime effect preparation | acceptance approval/resume | policy-denial/no-write scenarios | automated |
| Durable effects/recovery/replay | runtime + SQLite store | acceptance/completeness composites | uncertain mutation and corrupt/stale recovery tests | automated |
| Providers/capabilities | providers + compiler negotiation | fake/live-specific gates | unsupported capability and missing credential tests | mock plus historical opt-in live |
| Typed dataflow/control flow | compiler/runtime | v1 dataflow, parallel, matrix, router, loop, sub-workflow | conflicting memory and bounded-expansion tests | automated |
| Tools/process/network | runtime adapters + policy | acceptance and protocol resilience | path, network, overflow, cancellation, ambiguity tests | automated; container unavailable here |
| Artifacts/CAS/encryption/migrations | store/runtime | dedicated xtask gates | corruption, tamper, wrong-key tests | automated |
| Packs/extensions | core pack + CLI loader | reusable-pack and extension acceptance | digest/trust/protocol mismatch tests | automated |
| Observability/budgets | observability/runtime/store | acceptance and budget fixtures | exhaustion/reservation tests | automated |
| Instruction files | runtime instruction read/fingerprint | runtime regression and prompt-file fixture | exclusivity/path/size/encoding policy tests | partial; full command/recovery audit pending |
| Ordered variable files/origins | absent | none | unknown-field rejection | launch gap |
| Twenty task-specific DevOps examples | only broader v1/docs/completeness examples exist | `examples-verify` inventory | inventory enforcement | launch gap |
| Cross-repository site | sibling checkout absent | none for current SHA | not runnable | external blocker |

## Baseline and checkpoints

- `cargo xtask verify` was started from the initial checkout. `/usr/bin/time` was unavailable, so wall/RSS instrumentation could not be collected with that tool. Compilation overlapped the first bounded increment; its result is therefore a working-tree check, not immutable pre-change evidence.
- Pre-existing failures must be distinguished from failures introduced after the starting SHA. No source change was present at task start (`git status --short --branch` reported only `## work`).

## Remaining gates and next commands

1. Complete origin-aware ordered variable files and redacted origin diagnostics across DSL, packs, compiler, runtime snapshots, recovery hashes, CLI, schema, examples, and docs.
2. Add the twenty task-specific current-format examples and extend the machine-readable inventory/isolated runner.
3. Run `cargo xtask verify`, `acceptance`, `completeness`, all dedicated local gates, package, and secret scan against the final commit.
4. On a Docker-capable authorized runner, run container gates; on a bounded credentialed runner, run live OpenAI gates within 100 requests/200,000 tokens/30 minutes, recording actual model and usage.
5. Check out `opensourceops.github.io`, obey its instructions, remove the generated canonical-source boilerplate at its template source, and run its exact cross-repository build with `AGENTCTL_REPO` set to this checkout.
6. Push task branches and create linked draft PRs when authenticated remotes are available.


## 2026-09-06 continuation: current environment and baseline

- Framework code baseline: `ec7e220820ed169c005aff5344d81fe4d292cdb6` (current origin/main).
- Framework task branch: `codex/launch-readiness-20260906`, isolated worktree; prior doctor/ledger edits copied without changing the user's original dirty checkout. Prior reported commit dfa7062 is not present in this local object database; recovered the actual patch instead.
- Documentation baseline: `b1484ef7cc073e6cc756a5eca37a7ff3f6144cee` (current origin/main), task branch `codex/agentctl-launch-docs-20260906`.
- OPENAI_API_KEY is available to the runtime (presence only checked). GitHub CLI authentication works through the existing keyring. No credentials copied into files.
- Podman 5.8.2 engine responds; Docker executable is a compatibility wrapper. Existing unrelated containers are outside this task's ownership.
- Full baseline `cargo xtask verify` running against a separate untouched worktree at ec7e220. Logs retained in task workspace work/evidence/baseline-verify.log; an interrupted suite is never a pass.
- Documentation generated freshness baseline fails because checked-in imports reference earlier framework 2aeaa88 while the current framework is ec7e220. Cross-repository sync must use final framework SHA.
- Independent subagents own docs-site changes, examples/devops, and core/runtime source resolution. Primary owns CLI, pack-origin plumbing, suite budgeting and integration. No release tags, merge, package publication, public deployment or production mutations are authorized.

### Variable contract agreed before implementation

| Low to high | Source | Namespace / behavior |
| --- | --- | --- |
| 1 | spec.varsFiles, in declared order | vars, later file replaces a whole top-level value |
| 2 | spec.vars | vars, inline workflow default |
| 3 | agent.varsFiles, in declared order | vars, selected agent defaults |
| 4 | agent.vars | vars, compatible existing agent defaults |
| 5 | task.varsFiles, in declared order | vars, task-local overrides |
| 6 | task.vars | vars, compatible existing task-local overrides |
| 7 | repeated --vars-file, in command order | vars, explicit global invocation override |
| 8 | repeated --var KEY=JSON, in command order | vars, explicit global invocation override; last duplicate wins |
| Separate | spec.inputs → --inputs or --inputs-file → repeated --input | inputs, existing semantics preserved; explicit templates use inputs.KEY |

Objects, arrays, scalar values and null replace at the top-level key; missing keys retain lower defaults. No implicit deep merge, recursive includes, environment import or policy grants. Duplicate YAML keys are errors. Ordinary variables are non-secret configuration; dedicated secret references and existing encrypted state remain the secret mechanism. Reserved loop/matrix/foreach engine bindings cannot be invocation variables. File origins are declaring workflow/pack manifest parents, independent of the invoking cwd; explicitly supplied CLI files are relative to the invoking cwd and still subject to workflow read policy. File reads must be bounded regular UTF-8 inputs captured once before compilation, with fingerprints and redacted winning-source diagnostics.

### Confirmed gaps at initial inventory (historical)

- check/plan do not currently capture instruction files; runtime sends instruction templates without expansion; pack file origins are lost.
- Existing live guards are per-command and mostly post-run. Shared suite reservation and actual usage reconciliation are required before paid dispatch (100 requests, 200,000 total tokens, 30 minutes paid wall time, USD 25 estimate ceiling).
- doctor currently reports configured file/process secret references as available without probing; executable discovery does not establish container readiness. Strengthen honest readiness semantics.
- Twenty distinct useful DevOps scenarios and complete final hosted/container/live/site evidence remain required.


## Integration checkpoint, 2026-09-06

### Completed implementation (working tree; final source SHA pending)

- Source capture now spans parsing/compilation/CLI/runtime/recovery: instruction and ordered variable files are origin-aware, bounded regular UTF-8 files with content fingerprints, policy checks, redacted winning-source diagnostics, and persisted source snapshots. Templates expand with the inline typing/missing-variable rules. Replay/resume/fork use recorded captured sources. The full contract and legacy-history qualification are in [VARIABLES](../VARIABLES.md), [contract review](CONTRACT_REVIEW.md) and [feature evidence](FEATURE_EVIDENCE.md).
- `check`, `plan`, `doctor`, `explain`, provider/auth inspection and fresh run/retry/repair accept the same explicit variable/workspace options. Plan output shows an instruction digest instead of newly captured instruction bytes. Typed invocation inputs preserve their existing separate namespace.
- Schema migration 16 scopes provider tool-call IDs to durable effects, preserving independently completed/uncertain calls when separate agent responses reuse IDs. A v15-to-v16 regression and all 36 store tests pass.
- OpenAI cache routing no longer implicitly adds model-specific advanced cache options. One bounded baseline gpt-5-mini request exposed a definitive HTTP 400 for unsupported `prompt_cache_options`; a focused request-shape regression now passes. Explicit advanced cache options remain explicit. No successful live claim is made yet.
- All twenty new DevOps examples passed in isolated directories against the integrated working-tree binary. The suite has deterministic semantic assertions, governance identities, recovery checks, a machine-readable catalog, and four distinct opt-in OpenAI variants. Example 04 also has a real disposable build gate; no performance improvement is claimed from static review.
- Site importer removes the rendered canonical-source footer at its generating source and preserves provenance internally. Baseline browser/build checks pass; final cross-repository sync must pin the committed framework SHA.

### Baseline failures and current test evidence

- Untouched ec7e220 baseline `cargo xtask verify` passed compilation/tests through static examples, then failed supply-chain audit on h2 0.4.15 / RUSTSEC-2026-0258. This is a pre-existing failure, not a passing full baseline. Lockfile now selects patched h2 0.4.16; final audit remains required.
- Focused core/runtime source integration: core 72 + one compatibility test; runtime 96 passed with one pre-existing container-only ignore; final targeted source eight/instruction four reruns and Clippy passed.
- Four CLI source regression tests and 36 store tests passed. Provider request-shape regression passed. Deterministic shared-budget tests (6), CLI budget-wrapper tests (15), and integrated twenty-example semantic runs passed.
- Docs generated freshness was pre-existing stale against current main. Final clean-source metadata/route/hash checks have been strengthened; exact framework SHA remains pending.

### Ranked adjacent improvements

| Priority | Gap / implemented improvement | User value | Complexity / blast radius | Validation cost |
| --- | --- | --- | --- | --- |
| 1 | Honest `doctor` for first-run credentials and required OCI engine/image prerequisites | High | Low/medium; CLI plus existing process preflight API | Credential-free CLI and container cases |
| 2 | Discoverable `cargo xtask devops-examples` with clean-directory semantic evidence and enforced inventory | High | Medium; examples and xtask only | Twenty bounded fixture runs |
| 3 | Shared reservation/reconciliation for local and OCI paid-test commands | High; prevents suite/retry overspend | Medium; explicit opt-in harness plus conservative runtime input-token reservation | Deterministic budget failures, live ledger reconciliation |

`explain` belongs to the requested variables contract, rather than counting as a separate architectural enhancement. No scheduling, distributed state ownership or broad provider redesign was introduced. No speed or cost savings are claimed without before/after measurements.

### Paid-test controls and current accounting

- Runtime OPENAI_API_KEY remains in its provided environment mechanism; values are never saved in artifacts or configuration. API model-list GET succeeded using normal TLS. gpt-5-mini, gpt-5.6-sol and gpt-6-astra are accessible listed model IDs; GET alone does not prove a successful response.
- The task's one shared external SQLite allowance is `work/evidence/live-budget.sqlite3`. Every reservation uses a SQLite immediate transaction before dispatch. Keep this ledger through all retries; do not create a fresh allowance when a command fails.
- Explicit legacy live xtask commands require `AGENTCTL_LIVE_BUDGET` (absolute existing suite path) and `AGENTCTL_LIVE_MODEL`. Their copied fixtures receive the selected model and an explicit price schedule, without rewriting checked-in examples. New DevOps variants use `--live-budget` and the explicit matching `--model gpt-5-mini`.
- Harness limits map to runtime `maxProviderRequests`, `maxTotalTokens`, `maxWallTimeSeconds`, `maxCostMicrousd`; the shared ledger caps their aggregate at 100 requests / 200,000 input+output tokens / 1,800 seconds / 25,000,000 microUSD estimates. Reasoning tokens are an output subset and are not double-counted. Serialized UTF-8 bytes plus framing conservatively reserve prompt tokens before provider dispatch. Unknown/incomplete usage retains the reservation; definitive completed effects reconcile actual usage.
- Prices are dated estimates from official model pages, not a promise of final billing: mini $0.25/$2, sol $4/$20, Astra $10/$50 per million input/output tokens, with explicit cache prices. No claims for other providers or external production services.
- Current initial accounting: one actual baseline provider request, zero returned tokens, zero estimated cost; the unsupported-cache-option failure is retained as failure evidence. A preceding DSL validation failure made zero requests. Successful live and final aggregate evidence remain outstanding.

### Next commands / remaining release gates

1. Finish pack asset verification tests, generated schema/CLI references, harness integration and formatting; run full credential-free `cargo xtask verify`, acceptance/completeness and dedicated gates.
2. Commit integrated source; run package/container and explicitly bounded OpenAI gates on that source, including four distinct new workflows and an Astra compatibility case. Fix failures and only rerun affected gates.
3. Push task branch and open draft framework PR; collect required hosted Linux/macOS/Windows, container, security, package and SBOM results. No default-branch live workflow exists; runtime local credential and Podman are available.
4. Pin exact reachable framework SHA in docs, sync/build/verify with `AGENTCTL_REPO`, commit/push and open linked draft docs PR. Docs PR triggers validation; workflow dispatch/main push would deploy and are not authorized.
5. Record final source and any later evidence-only SHAs separately, artifacts/digests/model usage and remaining intentional limits. Release verdict remains **not ready** until every required gate is satisfied.


### Source-freeze checkpoint

Pack asset and transport hardening is complete: all declared UTF-8 assets are bounded, confined and fingerprint-verified; fresh archive downloads reuse workflow network/DNS/proxy/CA/timeout/response-size controls. Fresh HTTPS Git fetches are conservatively rejected because subprocess Git cannot enforce the pinned transport contract; contained local Git and exact cached commits remain supported. Eighteen focused pack/source tests and affected Clippy pass. Doctor uses nonblocking regular-file metadata inspection.

Store review additionally rejects tool-call/effect identity mismatches across runs/tasks/input digests and preserves populated schema-15 rows through migration16; six focused regressions pass. The CLI live wrapper now fails a successful child command if usage is incomplete or exceeds its reservation, preserving charged allowance. Seventeen deterministic wrapper tests pass. The container acceptance gate now explicitly covers captured instruction and variable sources, two attributed approvals, source deletion, resume and zero-effect network-disabled replay.

`cargo xtask generate`, `cargo xtask secret-scan`, full-workspace/all-target/all-feature Clippy with warnings denied and formatting passed before source freeze. Final full release gates are next, rather than inferred from these checks.

The corrected provider completed a real gpt-5-mini preflight: one successful request, 20 input +46 output tokens, estimated97 microUSD. Including the earlier definitive400, the shared allowance has charged2 requests,66 tokens,7 wall seconds and97 microUSD, with zero unreconciled reservations. This proves runtime credential and POST/TLS access; it is not final broad live evidence.

### Cross-platform checkpoint after source 266eb44360886aa3019e5d42eb9cd28d26f97246

Local full verification, 46 packaged CLI acceptance scenarios, completeness composites, source/package OCI acceptance and paired documentation verification passed. The local image build required the machine's existing trusted public CA through the existing ephemeral build-secret mechanism; runtime OpenAI TLS passed without an added certificate. The explicit live resource-budget gate passed with gpt-5-mini: one request, 18 input +42 output tokens, estimated89 microUSD, followed by a proven pre-dispatch request-budget denial. Shared totals are now 3 requests, 126 tokens, 10 seconds and186 microUSD, with zero unreconciled reservations.

Draft framework [PR7](https://github.com/opensourceops/agentctl/pull/7) and docs [PR6](https://github.com/opensourceops/opensourceops.github.io/pull/6) are open. Hosted framework container/security and macOS gates passed at266eb44; Linux and Windows failed in new fixture portability, so the release verdict remains **not ready**. Linux's cleared environment can leave `sys.executable` empty; Windows requires writable permission to remove fixture CAS blobs. The fixture now resolves its own installed interpreter and confines permission-repair cleanup to its disposable workspace. Five targeted regressions pass on macOS and real Linux Python, and all twenty local examples pass after the fix. Hosted Linux/Windows and RC preparation must rerun on the next source commit.

The docs hosted validation passed, but artifact inspection found upload-artifact omitted `.nojekyll`. A scoped docs workflow fix and a fresh paired source pin are being prepared. Remaining commands are the broad bounded OpenAI gates (Astra acceptance and Sol examples, including four distinct mini DevOps variants), final fixture/container execution, hosted platform/SBOM/RC gates, and refreshed paired documentation artifact verification. Previously proven unchanged paid cases will not be repeated.

### Live composite and adversarial checkpoint after 29429de

All twenty deterministic examples passed on clean source `29429de`. Astra local and OCI acceptance passed with four requests, 987 input +66 output tokens, tools/continuations and two zero-effect keyless replays. The first four independent Sol legacy examples also passed before the composite failed: five requests, 650 total tokens, estimated 3,576 microUSD. These successes follow the unchanged semantic assertions in the sequential gate; temporary histories were cleaned up and their individual run IDs were not retained.

The composite exposed a harness configuration conflict: raising per-agent output caps from 64 to 512 allowed four concurrent reservations to exceed its unchanged aggregate 1,536-token ceiling. The harness now caps each reservation at 384 for that fixture, preserving concurrency 4 and the aggregate ceiling. Deterministic regression coverage also proves the four independent live fixtures' caps remain unchanged. `examples-verify-live-openai-composites` continues the affected composite, local/OCI selective repair and four mini DevOps workflows without repaying for the successful prefix. Its report explicitly records a partial legacy inventory; it is not a replacement claim that a single full invocation passed.

The failed composite's incomplete accounting remains charged at its full reservation: 12 requests, 25,536 tokens, 240 seconds and 2,000,000 microUSD. It is not recorded as actual usage or a passing result. Before continuation, known reconciled totals are 12 requests, 1,829 tokens, 32 seconds and 16,932 microUSD; total allowance charged including the unknown reservation is 24 requests, 27,365 tokens, 272 seconds and 2,016,932 microUSD. The same SQLite ledger remains authoritative. The wrapper now retains numeric counters, run identity and effect statuses before reconciliation, without prompts, workflow values or outputs, so a later temporary-fixture failure preserves accounting diagnostics.

Final adversarial review added actual hostile instructions in tool data with a malicious second model request: the denied executor is never called, no forbidden file appears, policy/graph stay unchanged, and audit/effects retain task/run trace correlation. A variable-file-only recovery regression verifies retry rejection, selective unaffected-sibling reuse, fresh changed-operation approval, source deletion before resume, and zero-effect replay. Two Unix unreadable-file tests exercised real permission denial; a real Windows junction escape test is included for the next hosted Windows gate. No production policy or approval enforcement was loosened.

The corrected docs artifact for framework `29429de` passed hosted validation and independent ZIP verification, including `.nojekyll`; its code commit is c4ac6c795b6752e5cf6210e48afc54bc3d76886a and later evidence-only commit afa5f5315a3200dd2310db52a995dbfd36a719d7. Next: commit the harness/test increment, push and rerun required hosted/paired gates on that source, execute the focused live continuation and final Dockerfile fixture build, then append final evidence and verdict.

Hosted Linux and macOS full gates passed at `29429de`, while Windows found three further fixture issues: cleared-environment Git discovery, CRLF conversion of reviewed package/patch bytes, and an unclosed SQLite probe. Git is now staged by the trusted runner as an exact executable path/fingerprint in a fixed support file; ordinary workflow inputs and policy are unchanged. Byte-preserving writes and a package-specific Git attribute preserve the actual checksum assertion, and the probe closes its connection before cleanup. Eight portability regressions, all twenty local examples, eighteen accounting-wrapper tests, twelve xtask tests, full Clippy and secret scan pass for the next source increment. Hosted Windows execution of the new junction and fixture regressions remains required.


### Windows source confinement and final live-case contract checkpoint after 0eef26a

Clean-source `0eef26ae1d8034c8c8f49d5de4bdaa359a4ec494` passed all twenty local deterministic examples and the actual disposable Dockerfile build. Hosted Linux and macOS full CI/RC gates passed; Windows passed nineteen examples but exposed a remaining pipeline patch fixture issue. The failed hosted runs are [CI33996980808](https://github.com/opensourceops/agentctl/actions/runs/33996980808) and [RC33996985598](https://github.com/opensourceops/agentctl/actions/runs/33996985598); neither overall run is passing release evidence.

Security review found an intermediate Windows junction replacement race in the previous full-path open plus canonical-path check. The correction uses target-Windows cap-primitives/cap-fs-ext handle-relative no-follow component opens and opened-handle reparse checks, retaining ancestor handles. Sharing restrictions alone were rejected because attribute-only handles can mutate reparse points. Deterministic Windows tests stage held-parent rename, junction replacement/restoration, and in-place mutation/restoration of the already opened parent. Pinned Rust1.88 Windows cross-compilation/Clippy and eleven Unix source tests pass; actual Windows execution remains a required gate. Unsupported non-Unix/non-Windows capture fails closed, with no unsafe-code exception.

The focused Sol continuation completed all local composite/retry/repair and OCI selective-repair assertions at `0eef26a`: local17 requests/2,384 tokens/14,960 microUSD estimate and OCI5 requests/1,458 tokens/8,280 microUSD estimate, including keyless zero-effect replays. The invocation then failed in mini example01, so the full invocation remains failed. Retained local summary and numeric OCI ledger evidence preserve the completed prefix; the harness now writes its legacy completion summary before entering the separate DevOps suite.

Mini01 returned the correct missing-dependency diagnosis and source citation, but expressed `rootCause` as prose while the deterministic verifier required `missing_dependency`. The prompt and both fake/live output schemas now explicitly define the categorical codes; free-form recommendations remain prose and the existing semantic assertions remain unchanged. A keyless malformed-classification regression proves rejection at the structured-output boundary before downstream verification. The failed paid response remains recorded as one request/518 tokens/569 microUSD estimate; no previously successful Sol/Astra cases will be repeated.

Before the remaining four mini variants, known reconciled usage is 35 requests / 6,189 tokens / 80 seconds / 40,741 microUSD estimate. The earlier unknown composite reservation remains fully charged at12 requests/25,536 tokens/240 seconds/2,000,000 microUSD. Total allowance charged is47 requests/31,725 tokens/320 seconds/2,040,741 microUSD, with one unreconciled reservation. These charged totals are not actual usage. The original shared ledger and limits remain unchanged.

Docs source `e813d1650731a89bfb7cc83eca7762491dad7a5f` (evidence-only head `e8a8ca7d9c50d5a08f2c693e04fa489ac731b527`) pins framework0eef26a. Its paired suite and [hosted33997148424](https://github.com/opensourceops/opensourceops.github.io/actions/runs/33997148424) pass, deployment is skipped, and the actual280-file ZIP including `.nojekyll` verifies at SHA256 `c4451ea0d1b68529518076e81c9fc4412b119c06592a61514752d700ca7615a1`. The next framework source requires a fresh paired pin and artifact check.

Next: complete the Windows pipeline fixture correction and focused checks; commit/push the reviewed source; run required hosted platform/container/security/RC gates and only the four mini variants using the existing paid ledger; refresh final example/evidence records and paired docs. Verdict remains **not ready** pending these actual results.

The remaining Windows pipeline failure was reproduced with actual Git: host `core.autocrlf=true` rewrote a reviewed LF proposal into CRLF bytes during apply. The fixture now passes `-c core.autocrlf=false` only to its two apply commands; exact output-byte assertions remain intact. Nine portability tests, including the forced-host-setting regression, and examples03/04/07 pass locally. New Windows dependency advisories/licenses/sources audit, twelve xtask tests, full Clippy and secret scan also pass before the next commit.

Pre-dispatch review also aligned incident12's nonempty citations/computed duration and planner19's exact payload token with their existing verifiers. The official [Structured Outputs supported-properties guide](https://developers.openai.com/api/docs/guides/structured-outputs#supported-properties) was searched and fetched before the request-schema update; it documents array minItems/maxItems and enum constraints (the fine-tuned-model restriction does not describe the selected base model). No model, provider option or budget ceiling changed.


### Final mini compatibility checkpoint after db7b59b

Source `db7b59b63332dff2c2714e49196185402b5a46d4` passed all twenty isolated deterministic examples again, and the actual case04 non-root/network-disabled/read-only container build. Packaged binary SHA256 is `ae802ab15468e769297e175ce29407001b28c92055aff5431a878a7e967cdf4d`. The local paired docs gate at docs code `5f0cd561ae06b0da16fcc9c28efbbcc5836c5f60` passed64 imports/73 HTML pages/53 browser tests, with the one pre-existing duplicate-search skip. Hosted framework CI33998511938, RC33998517564 and container33998511945 are in progress; supply-chain-security33998511931 passed.

Live mini01/12/19 passed their actual semantic assertions on clean db7b59b: 7 requests, 3,406 input+output tokens and estimated3,199 microUSD, including two tool calls and typed planner/reviewer/executor handoffs. Their passing results are retained; no repeat dispatch is required. Example20 then received a definitive HTTP400 before any tool execution: the API rejected a quote-bearing JSON string literal in the strict `repair_config` content enum (request `req_0debb9da018040639ce61677f4abbccd`, run `run-01a073e4-9b31-7cd3-bfef-93ddb4942574`). This is one additional request with zero returned tokens, and remains a failed invocation. The official enum documentation was checked and does not document this quote restriction; the actual response is the compatibility evidence.

The narrow correction keeps the existing built-in workspace writer and exact value authority: the model writes only decimal token `30` to a fixed staging path, and the deterministic helper rejects any different bytes before serializing the existing exact JSON configuration. The final timeout30, loop/request ceilings, policy, approval, denial/no-effect and JSON semantic assertions remain enforced. No speculative regex, new provider option or broader tool grant is introduced. A final paid run will select only20 using the same ledger.

Known reconciled usage is now43 requests/9,595 tokens/109 seconds/43,940 microUSD estimate. The unchanged unknown reservation remains12 requests/25,536 tokens/240 seconds/2,000,000 microUSD. Aggregate allowance charged is55 requests/35,131 tokens/349 seconds/2,043,940 microUSD, with one unreconciled reservation. Reports now derive per-case live coverage from individually source-labeled attempts while retaining failures; combined coverage must not be presented as a single successful full invocation.

The representation correction passed ten helper regressions (including newline, whitespace, wrong value and quote-bearing token rejection with no final artifact), plus the complete deterministic case20 success/nonconverging/denial/keyless-replay journey. Its runner now asserts exact staging and final JSON bytes in addition to all prior semantic/loop/request checks. The paid01/12/19 workflows and instruction files are byte-unchanged from their successful db7b runs. Next source changes only case20-specific helper/runner branches, catalog evidence and recording, plus this ledger; the production binary remains unchanged.
