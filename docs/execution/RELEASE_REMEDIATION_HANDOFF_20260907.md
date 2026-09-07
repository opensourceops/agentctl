# Release and remediation handoff

## Source and review boundaries

- Framework code under test: `d3f4338a7735ff610947c1232ebf7797f584993d`; draft https://github.com/opensourceops/agentctl/pull/8.
- Documentation code under test: `87ff6cf135fe5a1cec6efddbc92953439e1ae660`, pinned to that framework source; draft https://github.com/opensourceops/opensourceops.github.io/pull/7. Documentation head `669f265b6331195a5c0f43aed8307a4019d2642c` adds only execution evidence to that tested site source.
- Standalone demo: https://github.com/Ompragash/agentctl-remediation-demo. Default branch remains `9149d9f278f7af399163619a2607d4a7fec7d89a`; reviewed bootstrap correction https://github.com/Ompragash/agentctl-remediation-demo/pull/1 at `59104af1f9fb16bb31a806a35af1508251066bc1` pins the tested framework.
- No implementation PR merge, release tag, GitHub release publication, production Docker Hub push or Pages deployment has occurred.

## Implementation and compatibility

Workspace/package metadata prepares 0.4.0. The native binary matrix retains Linux AMD64, macOS ARM64 and Windows AMD64 and adds Linux ARM64. Minimal and tooling images execute natively on Linux AMD64 and ARM64. Minimal remains nonroot/distroless with CA roots and an agentctl entrypoint; tooling adds the scoped shell/Git/Python helpers required by the standalone demo. Model execution receives neither a Docker socket nor a GitHub token.

Preparation verifies all package and image gates, assembles their retained bytes and validates disposable registry publication, rerun reuse and platform pulls. A signed manifest binds binary checksums, platform SBOMs, OCI source/config/provenance and native smoke records. Production publication verifies that complete bundle and promotes exact digests without rebuilding. Full-version mismatches fail; stable aliases advance only after both immutable flavors exist. Prereleases and older releases cannot move stable aliases forward incorrectly. Partial external writes use reconciliation, not an exactly-once guarantee.

The independent twenty-first example preserves the original twenty examples. It exports a complete standalone package, uses two bounded Astra/high roles with typed handoff and scoped writes, validates the patch, runs trusted application builds/tests and same-database Trivy scans, evaluates publication eligibility in a provider-free agentctl run, and publishes through a fresh repository-scoped CI boundary. External build/scan/GitHub effects are outside the replayed agentctl boundary. Captured agentctl output replay and selective repair have credential-free failure/denial contracts; CI rerun reconciliation is a distinct operation.

## Final source-specific validation

- Supply-chain/security https://github.com/opensourceops/agentctl/actions/runs/34091947756 and container/security https://github.com/opensourceops/agentctl/actions/runs/34091947743 passed on the exact framework source.
- CI https://github.com/opensourceops/agentctl/actions/runs/34091947786 passed on Linux, macOS and Windows, including its production SBOM.
- All four native image jobs and all four native package jobs passed in https://github.com/opensourceops/agentctl/actions/runs/34091976162, including independent downloaded artifact verification. The hosted complete release-preparation run passed, including disposable registry validation and retained attestation verification. Independent downloaded-bundle verification passed, including a separate exact-source/signer `gh attestation verify` with hosted-runner enforcement.
- Hosted documentation https://github.com/opensourceops/opensourceops.github.io/actions/runs/34092386026 passed: 140 browser checks, one unchanged generic skip, 94 HTML pages, no visible canonical/version branding, all twenty original downloads and one independent remediation download; deployment skipped.
- Actual site artifact `10007514654`: SHA256 `dccf9777467c25bcb5e4804efb67a8eccf14119527d1825e037a065ac75fcc31`. Nested remediation ZIP equals the clean standalone export: SHA256 `6d10813ad09b54ae75a6e7ecf6ea6ee4b4fb797c2961a70e1fb2497e4b8760ff`; manifest SHA256 `4c52252da1c30660110b35bf50eb78b79782eabfbb5032c80ff08d98d68d6d99`. All 34 tracked demo branch files match.
- Demo bootstrap contracts https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34092089701 passed; paid/remediation/publication jobs were skipped on this PR event.
- The final local budget correction passes all 25 coordinator regressions and deterministically fails with the old positional assertion. Earlier failures are retained in the execution ledger; they are not counted as passing results.

## Retained candidate digests

These digests were tested in the disposable registry, not published to Docker Hub. Preparation attempt 1 passed. Six tags were created, six reused on rerun, and four Docker platform/config pulls matched. The ten retained Actions ZIP digests and all 35 checksum-listed bundle files were independently verified.

| Flavor | Complete OCI index digest |
| --- | --- |
| minimal | `sha256:5e9ab1d769ab2fbceff70548eaafb6fdb2ccaa5e8a93d2a2c696492a643a53db` |
| tooling | `sha256:40765f2d796bf74b9a90cd5de24f506f9c8714d88ea23446dd865510ff1306c6` |

| Flavor / platform | Runnable manifest digest |
| --- | --- |
| minimal / linux/amd64 | `sha256:8e4d1df06eb33f0cac65dd8b8b911183f0d7161f9e5c4d69f4944f3b77acc0ce` |
| minimal / linux/arm64 | `sha256:d4204cabcc4d5fce25fedd1ae97d5b692fbb57b8bd47c8d795498ef4df83561b` |
| tooling / linux/amd64 | `sha256:aafc8ed461991b38a709ad48d8a6b8ee00f991e1d57c8b3e1efe3e86cb9d3e25` |
| tooling / linux/arm64 | `sha256:387dd19829171668e9708dde5ba95a06be37472c5cca3de5da9662b7294bbf66` |

| Native target | Binary SHA256 | Archive SHA256 |
| --- | --- | --- |
| aarch64-apple-darwin | `6da176a5a864fa4e5c45a42f9439c18d8f73a038c5c85acf535de875e51dd264` | `4137101bf8ad8cd321f53d70c75bd754c54d0299483e2446c26d0a66ad7d29fd` |
| aarch64-unknown-linux-gnu | `bc97586782892f455b6ae334dbb9ec6379506b1dc9612dca3764ba8f4e64ae1c` | `2ed77661d9b2f173e8d4c77ca6b9e380c8110576f7615cb869bcf5f6a8ec5a1e` |
| x86_64-pc-windows-msvc | `562e560c3bf6c6dc212f4e694e66b6004963b165ff2a4d34cab70029e64a914b` | `65fac6c45f94a2f563d4c33147e0d598f117c4a80c0177ed1e65c2c4ffe39c53` |
| x86_64-unknown-linux-gnu | `34ac0db94e38138fcfc925525b36f00a6e00427393233fa26b61a6bb65ce244c` | `2e7cf9373c2690aa1926955e698fd2d93a03fc8ac7f61cbdb1080717f3961d5a` |

The retained `release-bundle.json` hashes all asset records and SBOM/provenance/scan evidence; SHA256 `47674bb5bf42bac0441ada0cf9fde77d69c9bc2aaca50459be72a2d8e0565aee`. Its Sigstore bundle SHA256 is `1a647efba30c1610e19900438347acf35567a2011152443f83c2b18b2fabf829`. Complete Actions bundle ZIP SHA256 is `bf57935aca76fa96800993a27bfa8506a84c19191645dcc0daae6888a234133b`. Independent verification report `native-release-34091976162/validation.json` SHA256 is `b25bac124f40e77aa2dec8d2d38e7a411bca5a314d1534be4d39faee3b52021f`. Raw local evidence is under `.release-evidence/release-20260906/`; the corresponding hosted run links above retain the original artifacts.

## Real fixture versus pending live evidence

The confined sample dependency is hash-locked Python urllib3 2.6.2, with a reviewed 2.7.0 fix. Actual before image `sha256:0ce04b7091645be5fa37c4baca9b4da7ebbedb65729869771286dacc74f4306e` and reviewed fixed-fixture image `sha256:51273d39bfec840c39ae72c4b3f5a310aeffc90e9380cc5fa030cf31c135625e` used the same captured Trivy database. Two application tests passed; targeted HIGH findings `CVE-2026-21441`, `CVE-2026-44431` and `CVE-2026-44432` were removed, zero new findings appeared, and 270 unrelated findings remain (273 before). Saved image layers, app/test/lock bytes, installed versions and DB hashes were independently qualified against the unchanged final example inputs. This is earlier real build/scan evidence for a trusted fixture, not a fresh final-SHA build or model-generated remediation.

The configured model is `gpt-6-astra`, with high reasoning separately declared. New task paid usage remains **0 requests, 0 tokens, 0 seconds and USD0 estimated cost**. The task envelope remains reserved at 30 requests / 40,000 tokens / 900 seconds / USD2 estimated, within the original shared ledger; prior uncertain charges remain untouched. The first skipped preflight lease was reconciled only against exact GitHub skipped-job proof. No actual new model-backed patch, CI-created remediation PR, or live no-paid reconciliation success is claimed.

Automatic approval review rejected updating the demo default branch without explicit approval. The five-file bootstrap correction is reviewable in demo PR1; it changes no application, dependency, Dockerfile or application-test bytes. Once that merge is explicitly approved and checks pass, reserve a fresh exact-run preflight lease, execute the candidate-container preflight, reconcile its actual receipt, then separately lease the full two-role remediation and perform the no-paid PR reconciliation. Keep the trusted-main guard and scoped publisher credentials intact.

## Operator release sequence for 0.4.0

1. Review and separately approve/merge the implementation PRs. Select the exact intended main commit and wait for its CI, container and security checks to pass.
2. Dispatch `release-prep.yml` on that exact ref with `source_sha` equal to it, `release_tag=v0.4.0`, and `attach_draft=false`. Inspect complete native/bundle/disposable-registry/signature evidence.
3. The operator creates and pushes `v0.4.0` at that reviewed commit. **A tag push alone does not create a draft.** Explicitly dispatch `release-prep.yml` on `v0.4.0` with the same source and `attach_draft=true`.
4. Verify all native binary downloads, checksums, image archives, scans, SBOMs, provenance and signed bundle are attached to the draft before publication. Inspect the successful preparation. A new preparation rebuilds artifacts; published promotion itself never rebuilds.
5. The operator clicks **Publish release**. `release-image-publication` validates the exact prepared assets and promotes them to `docker.io/opensourceops/agentctl`. Minimal tags: `0.4.0`, `0.4`, `latest`; tooling: `0.4.0-ci`, `0.4-ci`, `ci`. No broad `0` tag.
6. Set the demo `AGENTCTL_IMAGE` to the published tooling digest and run the documented published-image smoke. This final consumer smoke remains pending the actual release. Registry retries reconcile the existing published release; do not rebuild or replace immutable tags to hide a mismatch.

Configure names in each repository's **Settings → Secrets and variables → Actions**. Framework: variable `DOCKERHUB_USERNAME`, secret `DOCKERHUB_TOKEN` with push access to `opensourceops/agentctl`. Demo: secret `OPENAI_API_KEY`, secret `GH_TOKEN` restricted to demo Contents read/write and Pull requests read/write, variable `AGENTCTL_MODEL=gpt-6-astra`, and the post-release digest variable `AGENTCTL_IMAGE`. Names except the deliberately pending image digest were observed present; actual production push/publisher access is not yet proven. Secrets are not inherited or copied between repositories. Ordinary PR checks remain credential-free. The built-in read-only workflow token handles read evidence, with narrowly scoped write/OIDC grants only at release asset/signing jobs; model/build/test steps do not receive the demo publisher token.

Release preparation, source CI/security and paired documentation validation are complete. Actual live remediation and its CI-created PR/reconciliation remain blocked on demo bootstrap merge approval; production release/publication and the published-image smoke remain separate operator actions. The complete requested live-integration outcome is therefore not yet validated.
