# Release process

This process applies to agentctl releases carrying workflow API
`agentctl.dev/v1`. A release is not approved from local evidence alone.

## Required hosted checks

Push the review branch and open a pull request only after the local gates below pass. Configure branch protection to require these checks:

- `credential-free-ci / gates (x86_64-unknown-linux-gnu)`
- `credential-free-ci / gates (aarch64-apple-darwin)`
- `credential-free-ci / gates (x86_64-pc-windows-msvc)`
- `credential-free-ci / production SBOM`
- `container-security / container`
- `supply-chain-security / security`

The three platform jobs run `cargo xtask verify`, `cargo xtask acceptance`,
`cargo xtask completeness`, and `cargo xtask package`. The other jobs enforce
the Linux container contract, HIGH/CRITICAL image vulnerability policy,
production and image CycloneDX SBOMs, complete-history and checked-out-tree
secret scans, dependency policy, immutable action pins, and workflow lint.

The repository owner must enable required checks on the protected release
branch. Repository-local changes and green pull-request jobs do not modify or
prove that remote governance setting.

## Local preflight

Run without provider credentials:

```console
env -u OPENAI_API_KEY -u AZURE_OPENAI_API_KEY -u ANTHROPIC_API_KEY \
  -u GOOGLE_API_KEY -u GEMINI_API_KEY cargo xtask verify
env -u OPENAI_API_KEY -u AZURE_OPENAI_API_KEY -u ANTHROPIC_API_KEY \
  -u GOOGLE_API_KEY -u GEMINI_API_KEY cargo xtask acceptance
env -u OPENAI_API_KEY -u AZURE_OPENAI_API_KEY -u ANTHROPIC_API_KEY \
  -u GOOGLE_API_KEY -u GEMINI_API_KEY cargo xtask completeness
cargo xtask package
```

Reproduce the production binary dependency SBOM with the pinned generator used in CI:

```console
cargo install cargo-cyclonedx --version 0.5.9 --locked
cargo cyclonedx --manifest-path crates/agentctl-cli/Cargo.toml --format json \
  --describe binaries --target x86_64-unknown-linux-gnu --spec-version 1.5 \
  --no-build-deps
mv crates/agentctl-cli/agentctl_bin.cdx.json agentctl-production.cdx.json
```

Run `cargo xtask acceptance-container` when Docker or Podman is available. If the builder requires an enterprise CA, provide a protected PEM file through `AGENTCTL_BUILD_CA_FILE`; see [Container](CONTAINER.md). Never disable TLS verification.

Run checksum-verified actionlint against `.github/workflows`, then run Gitleaks against both `git log --all` and the checked-out tree. `cargo xtask secret-scan` retains the deterministic repository scan and verifies every action reference is a full 40-character commit SHA with an exact-version comment.

## Hosted artifact verification

For the candidate workflow run:

1. Confirm every required check is green and was executed for the candidate commit.
2. Confirm the three `agentctl-<target>` package artifacts exist. Extract each artifact and verify its binary against its packaged `SHA256SUMS`.
3. Confirm `agentctl-production-sbom-cyclonedx` exists, parses as CycloneDX JSON, and its file SHA-256 matches the job summary.
4. Confirm `agentctl-image-sbom-cyclonedx` exists and parses as CycloneDX JSON.
5. Record the GitHub artifact digests emitted by `actions/upload-artifact` and the local image digest emitted by the container job.
6. Confirm no workflow artifact path includes `.release-evidence`, a database, provider credential, or live-response evidence.
7. Manually dispatch `rc-release-preparation` for the exact candidate commit and verify all three RC packages before creating a tag.

## Failure handling

- Platform failure: reproduce on the named OS/architecture; do not waive a matrix leg.
- Secret-scan finding: stop, revoke any real credential, remove it from the complete history using the repository's incident procedure, then rerun both history and tree scans.
- Dependency or image finding: review the advisory and remediate or document an explicit time-bounded exception before release. The default HIGH/CRITICAL image gate ignores only unfixed findings.
- SBOM failure or missing artifact: treat as a release failure. SBOM generation is not best-effort.
- Container CA failure on `main` or a manually dispatched run: configure only `AGENTCTL_BUILD_CA_PEM` as a protected repository/organization secret. Pull-request runs intentionally cannot receive it. Do not use insecure Cargo, Git, curl, or container flags.

## Release decision

Promote an exact commit only after it has all required hosted checks and
artifacts. Shipping workflow API `agentctl.dev/v1` does not imply a 1.0 CLI,
crate, storage, provider, or long-term-support contract.
