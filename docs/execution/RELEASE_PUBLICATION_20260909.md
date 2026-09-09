# Release publication follow-up — 9 September 2026

The user requested fixing all remaining release and docs publication issues after merging framework PR13 and docs PR9. This follow-up authorizes publication of the reviewed release and its published-image consumer validation.

## Immutable source and retained evidence

- Runtime/package/image source: `7dee64e1d1b6fd38c1883d0d235d712590c30fbe`, merged through PR13 at `8c9c86d7a3b4017a050f530a14783ba873ebe6c7`. Subsequent operational and evidence changes do not rebuild or relabel these artifacts.
- Complete native preparation: [34196038209](https://github.com/opensourceops/agentctl/actions/runs/34196038209), all four packages and all four images passed; signed bundle independently verified. Bundle ZIP SHA256: `3a5c22e50a54600055decb7d99100b39f0482d48cbc20bca6ca87ce19694e3c2`.
- Actual candidate live remediation: [34195429813](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34195429813), 3 gpt-6-astra/high requests, 4,290 tokens, $0.089960 estimated. Keyless network-disabled replay produced identical recorded outputs/artifacts and zero fresh effects. Provider-free outer CI reconciliation: [34196095845](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34196095845).

## Completed corrections and current gates

The obsolete unpublished `v0.4.0` tag object `622424f8e6d6da7f28e586505648b147a100909f` pointed to `0d4542cccdbd22cb96e3dec25cdffb66d0938ced`. Its draft release 384291529 contained eight superseded assets. Before replacement, the full remote metadata and annotated tag were retained; all eight assets matched retained old preparation bytes and API checksums. The tag was corrected with an exact old-ref lease to annotated object `0954fd27e365a1ef3c6b6e1f2e779669cd02e02b`, pointing to reviewed source `7dee64e`. The stale assets were removed from the unpublished draft and complete verified asset attachment started. Publication waits for complete remote asset verification.

Docker Hub's public repository lookup returned 404 and anonymous registry manifest lookup returned 401. Actions has publishing credential names configured, but no local Docker Hub credential can establish exact repository access. The narrow manual registry preflight uses that existing CI credential, the official namespace-scoped API, and anonymous public verification. Only an explicit `create_if_missing` dispatch can create the exact public release repository. It does not change existing visibility, permissions, tokens, or organization settings. Authentication, denial, private metadata, and uncertain creation fail closed. No image is pushed by this preflight.

Docs Pages run34384620405 failed twice before site validation because the unrelated Google Chrome APT repository had inconsistent signed metadata and package bytes. Docs PR10 isolates Playwright dependency installation to the existing signed Ubuntu sources without relaxing integrity checks. Its full gate and actual deployment are tracked in the docs repository.

## Operational validation

The registry preflight and existing release contracts passed 67 tests. The new manual workflow and changed publication workflow passed actionlint1.7.7. Independent review caught and corrected a shell pipeline failure-masking issue; a regression now executes the actual workflow body and checks that Python exit7 propagates through tee.

The publication transport install now selects only the existing signed Ubuntu source, fresh temporary lists, and an empty extra-source directory. The actual workflow shell passed in disposable Ubuntu24.04 ARM64 after an injected unrelated repository caused ordinary APT update to fail; Skopeo1.13.3 installed, and all source files remained byte-identical. No TLS, signature, checksum, or permission settings were relaxed. Hosted publication can retry this operational workflow from main while checking out the original signed source for all bundle and publisher validation.

## Budget and next commands

No new provider requests have been made by this publication follow-up. The existing shared task allowance remains 30 requests / 40,000 tokens / 900 seconds / $2 estimated, with 20 requests / 19,826 tokens / 241 seconds / $0.474145 charged or reserved. The unresolved earlier 3-request / 8,000-token / $0.20 reservation remains charged. Do not reset the ledger. A published-image smoke must acquire a lease from the remaining allowance before dispatch.

Next: merge the tested operational preflight, dispatch its read-only check, create the exact missing public repository if permitted, finish and verify all36 GitHub assets, publish the GitHub release, verify trusted promotion and public image digests, then lease and run the published-image consumer workflow and reconcile actual usage. Preserve any failures as evidence. Documentation must pass its complete gate, deploy, and match the pinned framework source and downloadable packages.

Release verdict at this checkpoint: not yet published; current runtime validation is complete, delivery gates remain active.
