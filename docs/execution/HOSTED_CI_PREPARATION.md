# Hosted CI preparation

Prepared: 2026-07-22, Asia/Kolkata.

Status: **workflow syntax/lint validated; hosted dispatch pending**. The files are configured locally and have not been pushed or dispatched.

## Workflow inventory

| Workflow | Trigger | Hosted purpose |
| --- | --- | --- |
| `credential-free-ci` | push, pull request, manual | Rust 1.88 full verification, acceptance, and packaging on Linux x64, macOS arm64, and Windows x64; production CycloneDX SBOM |
| `container-security` | push, pull request, manual | Linux x64 OCI build/runtime acceptance, Trivy 0.72 vulnerability gate, image CycloneDX SBOM |
| `supply-chain-security` | push, pull request, manual | full-history/tree Gitleaks 8.30.1 scans, synthetic detection proof, cargo-deny 0.20.2, deterministic scan, actionlint 1.7.12 |
| `rc-release-preparation` | manual | exact-commit three-platform RC verification, acceptance, packaging, and artifact digests |

The selected standard runner labels are `ubuntu-24.04` (x64), `macos-14` (arm64), and `windows-2022` (x64). Every external action is pinned to a full commit SHA with a nearby exact-version comment. Workflows grant only `contents: read`.

## Repository-owner preparation

1. Push the branch and open a review PR.
2. Enable GitHub Actions if repository or organization policy currently disables them.
3. Allow the pinned GitHub, Anchore, and Aqua actions, or approve their exact SHAs under the organization action policy.
4. Require the checks listed in [Release process](../RELEASE_PROCESS.md) on the protected release branch.
5. Optionally define protected secret `AGENTCTL_BUILD_CA_PEM` only when the hosted build network uses a private CA. Do not configure provider API keys for these workflows.
6. Retain artifacts for at least the configured 14 days and record workflow URLs/digests in the RC evidence.

The optional CA is written to a mode-restricted runner temporary file, mounted into the builder as `agentctl_ca`, combined with public roots only on tmpfs, and removed in an `always()` cleanup step. It is not a Dockerfile argument, image environment variable, ordinary build context file, or uploaded artifact.

## Local validation already completed

- actionlint 1.7.12, downloaded with its upstream SHA-256, reported no workflow errors;
- the deterministic action-pin scanner accepted every `uses:` reference;
- Gitleaks 8.30.1 complete-history and tracked-tree scans found no leaks, and a generated synthetic credential was rejected;
- the secure CA secret-mount build completed locally through Podman, followed by the full non-root/read-only OCI acceptance suite;
- checksum-verified Trivy 0.72.0 found zero fixed HIGH/CRITICAL findings in the current image and generated valid CycloneDX JSON.

These are local results. No GitHub workflow run, Linux x64 package, hosted macOS result, hosted Windows result, hosted artifact digest, or required-check result is claimed yet.
