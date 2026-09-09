# Release remediation handoff — 8 September 2026

The live OpenAI blocker and stale README token-limit sentence are resolved. Fresh model responses are not deterministic; compiled orchestration, captured execution, compatible reuse and replay retain their documented boundaries. Production publication remains a separate operator action.

## Source and review identities

| Deliverable | Exact tested source / review |
| --- | --- |
| Framework | `7dee64e1d1b6fd38c1883d0d235d712590c30fbe`, [PR #13](https://github.com/opensourceops/agentctl/pull/13) |
| Documentation | `2f2e7b03870899c9cc6f425e9d9eb8c005b15d99`, [PR #9](https://github.com/opensourceops/opensourceops.github.io/pull/9); framework pin remains `7dee64e` |
| Demo trusted main | `dfd8ba73af22eb9e121f0923e89d162c0b9bcf64`; maintenance PR #9 merged |
| Actual generated fix | [Draft PR #10](https://github.com/Ompragash/agentctl-remediation-demo/pull/10), head `458ea6480616ae8a5c9890646e466a577d762ca7`, validated tree `00de48e65a607cb471757028f4a2f3ba17b4879e` |

Docs evidence-only head is `86ac4876ecf14ced90b9128faf658a6674d6e731`; its subsequent hosted run 34197347005 also passed. Later evidence-only commits do not replace these code-under-test identities. The original outer checkout's unrelated CLI/generated-reference edits and untracked files remain preserved.

## Changes and evidence

An isolated API comparison changed only tool-generation strictness and produced the complete reviewed multiline contents after constrained generation repeatedly returned incomplete responses. The new explicit boolean `agent.providerOptions.toolStrict` applies to OpenAI/Azure, defaults to true, and controls generation only. Preflight and implementer opt out; complete argument schemas, strict final output, policy and approvals remain enforced. The README now reflects the actual 4096-token preflight/implementer and 2048-token analyzer ceilings.

Review also exposed a runtime authority defect: a provider could name a globally registered tool outside the current agent's tool list. Runtime now rejects that before lookup/dispatch. A hostile-provider regression fails on old code and proves zero forbidden writes with the fix. Tests also cover option validation, fingerprint changes, durable approval/resume, recorded continuation and replay.

| Gate | Result and evidence |
| --- | --- |
| Local full verification | `cargo xtask verify` passed at clean `7dee64e`, with complete pinned Python dependencies |
| Existing twenty examples | All 20 passed from clean directories; report SHA256 `58fb9485e7cb61d2a23ab66b11a3b77f2eb23ee0198cde772410db55c45a584a` |
| Framework CI | [34195126536](https://github.com/opensourceops/agentctl/actions/runs/34195126536): Linux, macOS, Windows and production SBOM passed |
| Container/security | [34195126487](https://github.com/opensourceops/agentctl/actions/runs/34195126487) and [34195126478](https://github.com/opensourceops/agentctl/actions/runs/34195126478) passed |
| Real native preflight | 2 requests / 1208 tokens; exact tool input and strict output passed |
| Real native recovery | Keyless OS-network-denied replay after instruction-file mutation; injected downstream assertion failure and executed selective repair reused the live agent result with zero provider/tool calls |
| Full live container workflow | [34195429813](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34195429813) passed: real builds/scans, two Astra roles, scoped writes, tests, rescan, eligibility, replay and draft PR |
| Provider-free reconciliation | [34196095845](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34196095845) reused the same branch, fingerprint, validated tree and PR; paid jobs skipped before steps |
| Generated fix PR checks | [34196000156](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34196000156) passed without model credentials |
| Docs | Local paired and [hosted 34195633694](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34195633694) passed; 140 browser cases, one unchanged generic skip, actual artifact/package parity verified |
| Native release preparation | [34196038209](https://github.com/opensourceops/agentctl/actions/runs/34196038209), passed with attachment disabled; all native packages/images, signed bundle and disposable-registry evidence independently verified |

Both native tooling images passed the synthetic scanner/scripted-provider OCI contract, including 19 rejected writes without tool dispatch, failed validation preventing publication, compatible reuse and zero-effect replay. This evidence is separate from the actual OpenAI/Trivy workflow. An injected terminal failure is not claimed as a process crash or same-run resume.

## Actual remediation outcome

The live roles made 3 `gpt-6-astra` requests with high reasoning, using 3261 input plus 1029 output tokens (4290 total; 426 reasoning tokens already included), 28 seconds, and USD 0.089960 estimated. They wrote exactly `requirements.in` and `requirements.lock`, upgrading the deliberately vulnerable `urllib3` fixture from 2.6.2 to 2.7.0 with its reviewed wheel hash. Both application test runs passed two tests.

The pinned Trivy scanner used one retained database snapshot, SHA256 `fc8340c6b58c86e8cc14f4791cb4bc1d6ee578548185c9fbb54ecd0fcd94e04a`. All three target advisories (`CVE-2026-21441`, `CVE-2026-44431`, `CVE-2026-44432`) disappeared, no findings were introduced, and 270 unrelated findings remain disclosed. This is a bounded dependency fix, not a claim that the sample image is vulnerability-free.

- Candidate tooling image: `sha256:f103338afe73139372157c5dcd282f1f0f4a61cf2bdc7ab6ffc4728b907cd9e2`.
- Before application image: `sha256:09542889af3fd998fa37d4170365fb008e0d17c07ac9068813ff3821b609d87e`.
- After application image: `sha256:a558abe12b502619a35f46107da2c67486d5c25ee96c4eb60494c43eb3819eb0`.

The actual agentctl run is `run-01a07fc1-b527-7f82-b188-1cb87fd964e5`; replay is `replay-01a07fc2-21e3-7fa2-ab0d-450e9fb44b6d`. SQLite integrity, provider/effect results, budgets and CAS bytes match retained inspections. Remediation and eligibility replays run without a key and with `--network=none`, preserving all recorded outputs/artifact digests with zero fresh effects. Builds, scans and GitHub mutations remain trusted outer CI effects, handled through artifact identity and remote reconciliation rather than agentctl replay or exactly-once delivery.

## Evidence and accounting

Retained evidence is under `.release-evidence/release-20260906/` in the original workspace. The following independently verified reports are separate from source implementation:

| Report | SHA256 |
| --- | --- |
| `remediate-34195429813/live-verification.json` | `4dbfd5c0ef07ca230c12e3752d8e9e4fd7798ec78a09b0ac452f0c1815e3f556` |
| `remediate-34195429813/publication-verification.json` | `6ffd78a9f761f419a3640f41f63a46aa37d4f9bb3d4b7fead4bc13472b420d8c` |
| `reconcile-34196095845/verification.json` | `a35fd2e745557f72d3ee30f4a6703cd0c0145386cac64b85523e25859ac7cffd` |
| `live-preflight-tool-strict/repair-verification.json` | `7fea73b89445869060de5dccf2a75d5f6092add0b30a543b734c3c954f46906d` |
| `docs/hosted-7dee64e/verification.json` | `d3b25f9d31ad4e0fb201a327b14bded8236f838dfea4e9e2563e75b3f3151514` |

This correction used 6 new requests and 6034 tokens across the diagnostic, native preflight and full workflow, costing USD 0.117880 estimated. Prices are configured public estimates, not invoice evidence. The prior task ledger remains active at 20 charged/reserved requests, 19826 tokens, 241 seconds and USD 0.474145. Those totals retain unknown run 15's complete 3-request/8000-token/120-second/USD 0.20 reservation; no uncertainty was erased. Its 17 reconciled requests report 11826 tokens and USD 0.274145, with a historical USD 0.002970 price correction recorded separately. An earlier one-request diagnostic remains separately charged to the original suite ledger. Do not initialize a new allowance or close the unresolved envelope.

Local validation initially hit an unrelated nested-workspace fuzz lookup and incomplete Python environment; unchanged gates passed in an external clean worktree with pinned requirements installed. The downloaded live artifact matched its advertised digest; its 1.37 GB Trivy database exceeded an initial local 1 GiB unpack bound, so extraction used streaming with a bounded 2 GiB limit and unchanged archive member/path/hash checks. These were evidence-environment failures, not hidden production passes.

## Remaining operator sequence

1. Review the framework and docs PRs; keep the generated remediation fix PR as a draft unless separately choosing to merge it.
2. Exact-source release preparation is complete. Use reviewed source `7dee64e1d1b6fd38c1883d0d235d712590c30fbe` and its verified preparation run; do not publish the old assets.
3. The existing `v0.4.0` tag still targets superseded `0d4542c`; draft release `384291529` is explicitly superseded and has only its old assets. Deliberately correct/retire that unpublished preparation before preparing a complete draft for the reviewed source. Never publish this stale draft or mix source identities.
4. Follow [the release process](../RELEASE_PROCESS.md) to attach the complete matching assets before the operator publishes. No production Docker Hub tags were pushed by this validation.
5. After publication, set the demo's `AGENTCTL_IMAGE` to the verified tooling digest and run the same scenario in published-image mode with a fresh lease from the existing allowance. Candidate success does not prove a production pull. Existing task headroom is 10 requests / 20174 tokens / 659 seconds / USD 1.525855 estimated; recheck it before allocation.

The final bundle ZIP SHA256 is `3a5c22e50a54600055decb7d99100b39f0482d48cbc20bca6ca87ce19694e3c2`. Independent `native-release-34196038209/validation.json` SHA256 is `02f4b0f25f2df7c75c35574844821cbd40aa8b7467a13c4d292ef5faaa42e4bb`. Its ten required artifact ZIPs, 35 checksum-listed files, two complete OCI indexes, source/signer-bound hosted attestation, six disposable tags, six rerun reuses and four platform pulls pass. A slow read-only bundle transfer was replaced by four validated HTTP206 ranges, preserving its 87,228,416-byte prefix and requiring the complete advertised hash; no build/test/provider execution was interrupted.

The live validation and release-preparation gates are passed. Production release, public site deployment of these new PRs, and final published-image smoke are not claimed complete.
