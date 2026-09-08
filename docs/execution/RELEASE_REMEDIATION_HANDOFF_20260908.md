# Release and remediation checkpoint — 8 September 2026

This checkpoint supersedes earlier current-state claims. Historical failures and receipts remain in [the execution ledger](RELEASE_REMEDIATION_20260906.md). The four requested actions are partially completed. **Not ready:** the stronger live preflight still fails, so full remediation, its model-generated draft fix PR, provider-free reconciliation, release publication and the published-tooling consumer run remain unproven.

## Sources and merged work

Framework [PR11](https://github.com/opensourceops/agentctl/pull/11) merged as `39ed172a8d7f2bc6c43e6ade7aeaa79ff01e1dfa`. Code under test is `bb8fb0a5a28d876fee26c49c49a8a5bf1c8390e2`. This handoff and the accompanying ledger checkpoint are evidence-only additions after that source.

Live-discovered fixes preserve exact multiline write authority, improve the preflight to exercise both real schemas, retain uncertainty for missing usage, distinguish actual incomplete-response reasons and prevent dispatch of partial tools. The latest source raises preflight and implementer response ceilings to4096 while keeping the proven analyzer at2048. Preflight defaults are3requests/16000total tokens/90seconds/USD0.60 estimated; full remediation remains4requests/20000tokens/USD1. Model `gpt-6-astra`, high reasoning, `store:false`, tool grants and recovery boundaries remain unchanged. These settings have deterministic coverage; successful live implementation is not claimed.

Demo [PR8](https://github.com/Ompragash/agentctl-remediation-demo/pull/8) merged as `316086796dc95b960c797ee4234eebe9519f4602`, pinning frameworkbb8. Its35 tracked export files (34payloads plus manifest) match the clean package. Maintenance PRs are not model-generated remediation PRs.

Docs [PR7](https://github.com/opensourceops/opensourceops.github.io/pull/7) merged as `cff5095b0957459af5646f04050c4e2a046f1c38` and deployed through [34182285736](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34182285736). All116 checked public files match the retained deployment artifact at framework0d4542c; visible canonical boilerplate is absent. This does not establish deployment of the laterbb8 correction.

## Passing source-specific gates

| Gate | Evidence |
| --- | --- |
| Linux/macOS/Windows CI and production SBOM | [34183776957](https://github.com/opensourceops/agentctl/actions/runs/34183776957), passed |
| Supply-chain/security | [34183776934](https://github.com/opensourceops/agentctl/actions/runs/34183776934), passed |
| Container/security | [34183776924](https://github.com/opensourceops/agentctl/actions/runs/34183776924), passed |
| Four native packages, four images and signed bundle | [34184038972](https://github.com/opensourceops/agentctl/actions/runs/34184038972), passed; attachment skipped |
| Paired documentation source/evidence head | [34184232135](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34184232135), [34184888790](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34184888790), passed; deployment skipped |
| Demo maintenance contracts | [34183982402](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34183982402), passed; paid jobs skipped |

Local example verification passed18 contract tests,13 adapter tests and66 actual OCI CLI invocations, including19 malformed writes denied before tool dispatch, with zero write effects, exact positive echo/output, selective repair and zero-effect replay. Full source CI also covers the prior35 provider tests, six runtime completion/terminal cases, two accounting/recovery regressions and26 coordinator cases. Paired docs validation passed140 browser cases with one unchanged generic skip,94 pages and all21 packages.

Evidence paths below are relative to `.release-evidence/release-20260906/` in the original checkout; raw artifacts are retained locally, with hosted run links above. Complete bundle validation independently checked all checksums and native relationships, source/signer attestation, both complete OCI indexes, six disposable tags, six rerun reuses and four platform pulls. No Docker Hub publication is implied.

| Retained artifact/report | SHA256 |
| --- | --- |
| `native-release-34184038972/native-evidence.json` | `f9faed3d23e290ca7b3b96e45b1724f5620823ba06e4a59fa59b1b9a892f70c9` |
| `native-release-34184038972/validation.json` | `8c5d5b3be67e559aa97588fee488f40fc65a546e118ab3e10cd72db34331b68f` |
| Complete bundle ZIP,254203465bytes | `967f6062b6e4e470a828f50afc685f0613d3cdd8ee98de56de5c2ea92387ca0c` |
| Signed release manifest | `f78f1e4f1034cba8cbec7f78fd4f0bf3ed097e22ace7cdf795c2fcdc45372591` |
| Sigstore bundle | `113e234c82e894844be168aa748d5ad21c479e5c2519f1657e78b19529c2583e` |
| `SHA256SUMS` | `43505e84ac9e2e571435fd794fa66c6cede635fdf1242eac11ab077d0a65f3fd` |
| `docs/final-docs-report-bb8fb0a.json` | `f28555d88228e2a87a950e50f5c33fbe18dbdc5a7e9c000449ab7e5ba9641061` |
| Actual bb8 docs artifact ZIP | `c069d43f044e062cdf6f0e2eabcb171ecc29ce1e57fd3ad22009a5c94d93abf6` |
| Clean demo ZIP | `5853c6b30e97fb5efe179f21953ce4a2a6bfd8fc20823ce8aca55e86d05838c6` |
| `preflight-22-wire-audit/report.json` | `47fef087e1301a5abc8c563eaec783a3fd304f4259b35cc464612cc7503087fe` |
| `preflight-local-25000/verification.json` | `4e0608e4492815f473c29d91b9907885eb8a562c43c8127630da13b72524f8fa` |

Final prepared minimal index: `sha256:d5025ebc91237276ef477605e11aeb35ce062b8b5b78925608bcb89c4c8170ca`. Tooling index: `sha256:435d2ab00be4f6e5eb655339d369d8a7174ae7155bb6b26997aeb22ac718e216`. These were verified in a disposable registry only. Preserve failed/partial download and opaque-build-record inspection evidence; successful byte verification did not require weakening any release gate.

## Live blocker and budgets

The simple earlier echo preflight passed. Full attempts failed before writes on provider schema rejection; subsequent schema corrections have deterministic coverage. The stronger exact multiline preflight still fails. Current-source hosted [34185368189](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34185368189) returned explicit `max_output_tokens` on the first request at4096, before echo or continuation. Required usage counters were numeric0/0; verified receipt reconciles1request/0reportedtokens/3seconds/USD0 estimate.

Offline audit reconstructed the unchanged Rust wire request and verified all strict schemas, exact LF literals and19 negative cases. No mapping defect was demonstrated. A runtime key was available locally; a TLS-verified model metadata GET returned200 with no environment proxies. One isolated25000-output-token diagnostic followed [official reasoning guidance](https://developers.openai.com/api/docs/guides/reasoning#controlling-costs), using the verified bb8 macOS ARM64 package and pure echo. Its temporary2request/60000token/180second/USD4 allowance was reserved directly against the original shared ledger, without changing repository defaults.

That diagnostic also failed on its first response with explicit `max_output_tokens`: `resp_030541145402578f016a9f8e06357487d2a5b7b01b5bacfd91`. Required counters were numeric0/0, no tool dispatched and no runtime reservation remained. Harness elapsed4seconds (runtime3) was reconciled. Empty normalized output does not establish absence of raw partial upstream items; the adapter intentionally omits incomplete items. Keyless offline replay refused the pending downstream task with exit4, so successful replay of this failed workflow is not claimed. The cause of the reported exhaustion remains unresolved. No arbitrary further cap increase or unchanged paid retry is planned.

Across this remediation task plus the local diagnostic,13 provider calls were observed. Twelve reconciled calls report5792tokens and USD0.156265 runtime estimates; the separate historical cache-write correction adds USD0.002970, giving USD0.159235 known estimate. These are not invoices. Unknown preflight15 retains its entire3request/8000token/120second/USD0.20 lease. Older15/18 cannot be retroactively classified because their adapter discarded incomplete reasons.

The task ledger remains at14charged/reserved requests,13792tokens,199seconds and USD0.356265 within its unchanged30/40000/900/USD2 envelope. The local diagnostic is charged separately in the original ledger. Original aggregate charged/reserved totals are97requests,81926tokens,1284seconds and USD4.047740, including the entire task envelope and historical charges. Reservation totals are not observed calls or actual invoice spend. Do not reset either ledger, release unknown usage, or allocate a duplicate envelope.

## Approval and publication state

Docs [PR8](https://github.com/opensourceops/opensourceops.github.io/pull/8) is open/non-draft at `878560e75d477a4f2d4eb076ccfb4753049dd4e8`; tested docs source is `826071bf92f0bf975757f27db6201b22f686dd59`. All checks and artifact verification pass. Automatic approval review rejected its merge because it triggers public Pages deployment. A new explicit question naming PR8 remains pending. The earlier “go ahead” was applied to PR7 and its deployment; no rejected PR8 action was bypassed.

The task-created unpublished v0.4.0 tag was approved and corrected to0d4542c, but remains superseded bybb8. Draft384291529 is unpublished and labeled “Superseded candidate—do not publish”; eight old0d assets remain. Its obsolete upload process was stopped. No Docker Hub promotion occurred. Do not publish this partial draft, silently reuse its source identity forbb8, or replace any published immutable version.

## Next steps from retained work

1. Resolve the exact PR8 Pages approval. If approved, merge only its verified head, wait for normal deployment, and compare actual public bytes/source metadata with the deployed artifact.
2. Investigate the reproducible model/schema interaction using retained response IDs and `OPENAI_INCOMPLETE_REPRO.md`. Require new evidence before a paid retry. A narrow correction needs focused tests and a separately reserved diagnostic; preserve model/high and exact write authority unless the user explicitly changes that contract.
3. After a passing stronger preflight, reserve full remediation against the existing task ledger and current trusted demo main/run identity. Verify actual model writes, deterministic validation, a draft fix PR, and the provider-free reconciliation path. No such model-generated PR exists yet.
4. Only after live gates pass, prepare source-specific final release evidence, resolve the corrected tag/partial draft for that exact source, verify complete signed assets, promote the exact OCI bytes, then execute the full published-tooling consumer workflow. The already verifiedbb8 bundle needs no rebuild if release source remainsbb8.

Read-only continuation commands:

```sh
gh pr view 8 --repo opensourceops/opensourceops.github.io
gh run view 34185368189 --repo Ompragash/agentctl-remediation-demo
gh run view 34184038972 --repo opensourceops/agentctl
gh api repos/opensourceops/agentctl/releases/384291529
```

The original checkout's existing CLI/docs edits remain untouched. All unblocked source, native and paired-site verification is retained. The live blocker and Pages approval prevent completing all four requested actions; the supported release verdict remains **not ready**.
