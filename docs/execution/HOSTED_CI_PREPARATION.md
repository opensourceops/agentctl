# Hosted CI validation

Prepared: 2026-07-27, Asia/Kolkata.

Status: **exact-head pull-request validation enabled**. Automatic workflows
run without provider credentials. The manual release-preparation workflow is
dispatched only after the automatic gates pass on the final candidate.

## Workflow inventory

| Workflow | Trigger | Hosted purpose |
| --- | --- | --- |
| `credential-free-ci` | push, pull request, manual | Rust 1.88 full verification, acceptance, framework completeness, and packaging on Linux x64, macOS arm64, and Windows x64; production CycloneDX SBOM |
| `container-security` | push, pull request, manual | Linux x64 OCI build/runtime acceptance, Trivy 0.72 vulnerability gate, image CycloneDX SBOM |
| `supply-chain-security` | push, pull request, manual | full-history/tree Gitleaks 8.30.1 scans, synthetic detection proof, cargo-deny 0.20.2, deterministic scan, actionlint 1.7.12 |
| `rc-release-preparation` | manual | exact-commit three-platform RC verification, acceptance, framework completeness, packaging, and artifact digests |

The selected standard runner labels are `ubuntu-24.04` (x64), `macos-14` (arm64), and `windows-2022` (x64). Every external action is pinned to a full commit SHA with a nearby exact-version comment. Workflows grant only `contents: read`.

## Repository-owner preparation

1. Push the branch and open a review PR.
2. Enable GitHub Actions if repository or organization policy currently disables them.
3. Allow the pinned GitHub, Anchore, and Aqua actions, or approve their exact SHAs under the organization action policy.
4. Require the checks listed in [Release process](../RELEASE_PROCESS.md) on the protected release branch.
5. Optionally define protected secret `AGENTCTL_BUILD_CA_PEM` only when `main` or manually dispatched hosted builds use a private CA. Pull-request runs do not receive it. Do not configure provider API keys for these workflows.
6. Retain artifacts for at least the configured 14 days and record workflow URLs/digests in the RC evidence.

The optional CA is written to a mode-restricted runner temporary file, mounted into the builder as `agentctl_ca`, combined with public roots only on tmpfs, and removed in an `always()` cleanup step. It is not a Dockerfile argument, image environment variable, ordinary build context file, or uploaded artifact.

## Local validation

- actionlint 1.7.12, downloaded with its upstream SHA-256, reported no workflow errors;
- the deterministic action-pin scanner accepted every `uses:` reference;
- Gitleaks 8.30.1 complete-history and tracked-tree scans found no leaks, and a generated synthetic credential was rejected;
- the secure CA secret-mount build completed locally through Podman, followed by the full non-root/read-only OCI acceptance suite;
- checksum-verified Trivy 0.72.0 found zero fixed HIGH/CRITICAL findings in the current image and generated valid CycloneDX JSON.

These remain local results and are not substitutes for the exact-head hosted
jobs. The independent candidate report records the hosted run IDs, job
outcomes, and artifact digests separately.

## Exact-candidate procedure

Automatic pull-request gates run when the candidate branch is pushed. After
they pass without a skipped required job:

```console
gh workflow run release-prep.yml --ref feat/framework-completeness
gh run list --workflow release-prep.yml --branch feat/framework-completeness
```

Wait for all three matrix jobs and record the run URL plus every uploaded
package digest. A later source change invalidates that evidence and requires
all applicable exact-head gates to run again.
