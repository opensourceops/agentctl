# Release publication follow-up — 9 September 2026

The user requested fixing all remaining release and docs publication issues after merging framework PR13 and docs PR9, including publication and published-image consumer validation. **Completed: v0.4.0 is published, its public images are verified, the docs are deployed, and published-image live execution, replay, and publication reconciliation passed.** This is a pre-1.0 release, not a stable 1.0 declaration.

## Source and artifact identity

- Runtime/package/image source: `7dee64e1d1b6fd38c1883d0d235d712590c30fbe`, merged through [PR13](https://github.com/opensourceops/agentctl/pull/13) at `8c9c86d7a3b4017a050f530a14783ba873ebe6c7`.
- Operational delivery changes: `63ff8588573b9e621306d69bcbca58adc9ee8221`, merged through [PR14](https://github.com/opensourceops/agentctl/pull/14) at `4df64c669b1d6e966375f1a1b830471bae795bf4`. This final ledger update is evidence-only. Neither change rebuilds or relabels the released source artifacts.
- [Native preparation 34196038209](https://github.com/opensourceops/agentctl/actions/runs/34196038209) passed all four native packages and all four images, signed bundle verification, disposable-registry promotion/reconciliation, and all four platform pulls.
- Bundle artifact `10044589565`, ZIP SHA256 `3a5c22e50a54600055decb7d99100b39f0482d48cbc20bca6ca87ce19694e3c2`.
- Release manifest SHA256 `7f42f01a2e87f46745d0d69a1f9d54cb25fa2b8400e6044a9515dfead0cf21ac`; Sigstore bundle SHA256 `48c4bb7ff44eddf29dc5999d767331ec9814bda0bc11352a834a7904d5f401d3`; SHA256SUMS SHA256 `f1f54ebdbe2e162e344098b748894a5fb760bfa92054ed4efdb3061d5ddf2606`.

## GitHub and Docker Hub publication

[GitHub release v0.4.0](https://github.com/opensourceops/agentctl/releases/tag/v0.4.0), release ID `384291529`, was published at `2026-09-09T19:01:56Z` with all **36 assets**. Every server-computed asset SHA256 matched the signed preparation. Downloaded manifest, signature and checksums matched byte-for-byte. The actual downloaded macOS ARM64 package passed version and credential-free hello execution in a clean directory. GitHub reports `immutable: false`; artifact verification does not claim server-enforced release immutability. Published tag and asset bytes must remain unchanged.

The obsolete unpublished tag object `622424f8e6d6da7f28e586505648b147a100909f` pointed to `0d4542cccdbd22cb96e3dec25cdffb66d0938ced`, with eight superseded draft assets. Full remote metadata and the annotated tag were retained, and all eight old assets matched retained bytes/API checksums before removal. An exact old-ref lease corrected the unpublished tag to annotated object `0954fd27e365a1ef3c6b6e1f2e779669cd02e02b`, pointing to reviewed source `7dee64e`. Complete signed assets replaced the stale draft before publication.

The initial public Docker Hub repository lookup returned 404. [Read-only registry preflight 34392476769](https://github.com/opensourceops/agentctl/actions/runs/34392476769) authenticated with the existing CI credential and correctly failed on the missing repository without mutation. [Explicit creation preflight 34392528156](https://github.com/opensourceops/agentctl/actions/runs/34392528156) created only the exact public `opensourceops/agentctl` repository and verified anonymous access. Existing visibility, permissions, tokens and organization settings were unchanged.

[Production promotion 34392612869](https://github.com/opensourceops/agentctl/actions/runs/34392612869) passed at the exact released source, promoting prepared bytes without rebuilding. All six public tags, all four platform manifests/config descriptors, and all four provenance manifests were independently verified anonymously against the signed bundle.

| Public tags | OCI index digest |
| --- | --- |
| `0.4.0`, `0.4`, `latest` | `sha256:54879d0f6f1543fc7a94ef124542ab6f2695c45d6cfde922a728f7a590e3df36` |
| `0.4.0-ci`, `0.4-ci`, `ci` | `sha256:36b6a3f944b51aca65e7a841d9be159a3decd52a525afca7fa9fd8a5d59e81d5` |

Both published ARM64 variants were pulled and executed with `--network=none` on native Linux ARM64, verifying their source, version, config IDs and repository index digests. An initial local verifier failure reflected Podman 5.8.2 returning a bare 64-hex image ID; normalization was restricted to exactly that representation, then all expected identities passed. This was retained as verifier evidence, not concealed or treated as a product failure.

## Live published-image execution and recovery

[Published-image consumer run 34393008761](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34393008761) passed from trusted demo source `dfd8ba73af22eb9e121f0923e89d162c0b9bcf64`, run 27/attempt 1. It used the public tooling index above and verified AMD64 config `sha256:c8242fc79850b41139e4cfe609269b5727be7d6fc8bd27912c0cccb239d5fbd1`.

- Real OpenAI `gpt-6-astra`, reasoning `high`: **3 provider requests, 2 tool calls, 3,276 input + 1,299 output = 4,575 tokens, 31 seconds, $0.103626 estimated**. Reasoning tokens are included in output tokens and counted once. Cost is configured public-price accounting, not an invoice.
- Remediation run `run-01a08790-ea40-73b2-b352-7c2db834dfb5`; replay `replay-01a08791-6338-7bf1-9aca-fb7a13f11b40`. Eligibility run `run-01a08791-7ed0-7ce3-9697-ea17b2d6570f`; replay `replay-01a08791-80a8-7220-bfd9-685ec9c87197`.
- Independent downloaded-artifact verification passed SQLite integrity/inspection parity, CAS and effect evidence, two scoped model-written files, exact recorded outputs/artifact digests, and **zero fresh replay provider requests or effects**. Replay was keyless and network-disabled.
- The fixture patch changed urllib3 2.6.2 to 2.7.0, passed builds/tests and same-database rescanning, and removed CVE-2026-21441, CVE-2026-44431 and CVE-2026-44432. The fixed Trivy DB SHA256 was `156b39d6acdbc6065c2864d1f68e3f81d355d47e7e18284b8588e0f161a06bf8`. **270 unrelated findings remain disclosed**; this is not a vulnerability-free application or production deployment claim.
- Publication reused the existing validated tree `00de48e65a607cb471757028f4a2f3ba17b4879e` and [draft demo PR10](https://github.com/Ompragash/agentctl-remediation-demo/pull/10), with fingerprint `9db362bf3f74b8ea4251ff35f41be762db6ea9ee3a9d212539740f1db5d02c84`.

[Provider-free reconciliation 34393474429](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34393474429), run 28/attempt 1, verified the exact prior publication artifact and reused the same branch/tree/PR. Paid jobs were skipped before execution, with **zero provider requests**. This is trusted outer CI publication reconciliation, distinct from agentctl resume and from transactional rollback.

Earlier unchanged-source evidence remains valid: [candidate live run 34195429813](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34195429813) used 3 requests/4,290 tokens/$0.089960 estimated; [CI reconciliation 34196095845](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34196095845) passed. The prior release ledger records keyless replay after instruction-file mutation and selective repair of an injected downstream failure reusing the successful live boundary without fresh provider/tool calls. All 20 deterministic DevOps examples passed from isolated directories.

## Documentation and delivery fixes

[Docs PR10](https://github.com/opensourceops/opensourceops.github.io/pull/10), reviewed source `7969fc1b1ad313990e7231467aad6647e9960f42`, merged at `f53941091e366b27c097cfd14bab2d043c9d130b`. [Pages deployment 34390286498](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34390286498) passed and deployed [the public site](https://opensourceops.github.io/agentctl/), pinned to framework `7dee64e`.

The full gate passed 140 browser checks with one unchanged generic skip. All 122 checked public files (including 92 HTML pages and 21 downloadable packages) matched the actual deployed artifact; all 346 nonhidden artifact files matched the reviewed PR build. Package manifests/payloads matched the pinned framework. Browser search returned results and the 390px tutorial had no horizontal overflow. No visible `Canonical source:` boilerplate or stale README token-limit sentence remained.

Earlier Pages run 34384620405 failed twice because an unrelated Chrome APT repository served inconsistent metadata/package bytes. The fix isolates Playwright dependency installation to existing signed Ubuntu sources using process-local APT configuration. The release transport installer received the same isolation. No `/etc` source changes or TLS/signature/checksum relaxations were made. The old artifact's CSS discrepancy was retained and characterized; current reviewed-build/deployment parity passed, without claiming whole-site parity with the old artifact.

Operational release contracts passed 67 tests and actionlint 1.7.7. Independent review found and fixed shell pipeline failure masking, with a regression executing the actual workflow body to verify exit 7 propagates through `tee`. The actual transport step installed Skopeo 1.13.3 in disposable Ubuntu 24.04 ARM64 after an injected unrelated repository failure, leaving source files byte-identical. Hosted [Linux/macOS/Windows and SBOM CI 34390077328](https://github.com/opensourceops/agentctl/actions/runs/34390077328), [container 34390077295](https://github.com/opensourceops/agentctl/actions/runs/34390077295), and [security 34390077283](https://github.com/opensourceops/agentctl/actions/runs/34390077283) all passed before PR14 merged.

## Retained verification and budget

Evidence is retained under `.release-evidence/release-20260906/` (local generated evidence; not all committed). GitHub run artifacts provide the corresponding downloadable execution evidence.

| Evidence | Artifact ID / SHA256 |
| --- | --- |
| Published live full evidence | `10120450471` / `2b7ad120e902fa968a46b81f6ec17ca6fb8f6d1e80e717b2d8fb856ce1dda4a8` |
| Publication bundle | `10120451522` / `edf6c6b550fa25385020867e5ed16730d5337e25f541aa67186013cd90e7813e` |
| Publication result | `10120463477` / `f5ebbfc425fe947177f0d89ec7e0f2a39cd574f4090c53de083354961447f193` |
| Reconciliation result | `10120565672` / `85434b6e0a85ac075aefe54b39ee9cdf23bd6ec6ece47cd4042881cfddc5a5d1` |
| Independent live verification JSON | `f26011147cfd9400c41728f07521d929d0dcd3d78e4983fc4edfa5fafbd23b24` |
| Actual live budget receipt | `b640183c1b9ddcf78e74a2055496934b3eb7df9ceb6f01ab5ef2c8a51db42ef1` |
| Deployed Pages artifact | `10119712279` / `7ce96b7dd581c69ac496dce179b4aa5063bbc62dc7925659cb82048aa7f2e308` |
| Independent docs deployment JSON | `322673fa4b505349acdddd0a28f0f36d76e498e3c899459f809953274db910df` |

Published run 27 reserved lease `4a1aaa70-60b3-45e8-87c9-74d8835343ac` before dispatch, bound to the exact source/workflow/run/attempt/job, with caps 4 requests/20,000 tokens/600 seconds/$1 estimated. Its verified receipt was reconciled into the existing task ledger. The task now charges or reserves **23 requests/24,401 tokens/272 seconds/$0.577771** against 30/40,000/900/$2. The earlier unresolved run 15 reservation remains charged in full (3 requests/8,000 tokens/120 seconds/$0.20); it is not passing evidence or zero usage. The original shared envelope has not been reset or prematurely released. No more paid reruns are required for these unchanged cases.

Reverification entry points: `gh release view v0.4.0 --repo opensourceops/agentctl`; `gh run view 34392612869 --repo opensourceops/agentctl`; `gh run view 34393008761 --repo Ompragash/agentctl-remediation-demo`; `gh run view 34393474429 --repo Ompragash/agentctl-remediation-demo`; and `gh run view 34390286498 --repo opensourceops/opensourceops.github.io`. Verify downloaded artifacts against the identities above before inspecting their content.

## Verdict and remaining boundaries

No release-publication blocker remains for the tested pre-1.0 v0.4.0 source. Deterministic graph execution, captured replay and conservative recovery are evidenced; fresh independent model generation may vary. Local SQLite ownership, bounded graphs, external scheduling, best-effort compensation and conservative ambiguous-effect handling remain intentional. OpenAI evidence does not establish live coverage for other providers, cloud accounts or production deployments. The demo PR remains a reviewable fixture artifact. The older unknown budget reservation remains an accounting uncertainty, conservatively charged, rather than a failure of the completed published-image gate.
