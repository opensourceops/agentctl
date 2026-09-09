# Prepare and publish a release

An agentctl release supplies native GitHub binary downloads and two Docker Hub image flavors. It does not publish crates to crates.io or deploy a service. Preparation builds and tests exact source; publication promotes the retained OCI bytes without rebuilding.

## Operator sequence

1. Review and merge the implementation. Select an exact source commit with the intended workspace version. Run the existing hosted CI, container-security and supply-chain-security workflows for that commit and wait for all of them to pass.
2. Dispatch **release-preparation** on a branch or tag pointing to that exact commit. Supply `source_sha` and `release_tag` matching `Cargo.toml`; leave `attach_draft` false for validation. The workflow rejects a source different from its selected workflow ref.
3. Inspect all four native packages, both image flavors on both native architectures, the disposable registry roundtrip and rerun evidence, scans, SBOMs and the signed bundle manifest. A failed architecture or required workflow prevents assembly.
4. Create the intended release tag at that reviewed commit. Dispatch the same preparation workflow on that tag with `attach_draft` true. The source must be an ancestor of upstream main. This mode creates or safely reuses a **draft** GitHub release and attaches the complete assets before publication. It does not publish the release or push Docker Hub tags.
5. Inspect the draft and verify the preparation run is complete and successful. Click **Publish release** yourself. This triggers **release-image-publication**, which verifies the tag, source, version, signed bundle, required artifacts and completed preparation before logging in to Docker Hub.
6. Retain the publication summary with release URL, source, native binary checksums, image index/platform digests and tag results. Run the remediation demo in published-image mode using the chosen tooling digest.

Use GitHub's Actions UI or the GitHub CLI. These commands derive the version from the actual checkout rather than copying a documentation version:

```sh
SOURCE_SHA="$(git rev-parse HEAD)"
RELEASE_VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
RELEASE_TAG="v$RELEASE_VERSION"
gh workflow run release-prep.yml --repo opensourceops/agentctl --ref YOUR_REVIEWED_BRANCH \
  -f source_sha="$SOURCE_SHA" -f release_tag="$RELEASE_TAG" -F attach_draft=false
```

After review and successful validation, the release operator creates and pushes the tag, then dispatches preparation with `--ref "$RELEASE_TAG"` and `-F attach_draft=true`. Creating a GitHub release without preparation fails with instructions to prepare complete assets. Published immutable release assets cannot be repaired by uploading missing files afterward. GitHub's [immutable release model](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases) is why assets are attached while the release is still a draft.

Preparation also accepts an existing exact release tag. An explicit commit can be validated before any tag is created; attaching a draft requires the operator-created matching tag. A new validation run rebuilds artifacts and may produce different provenance. Do not replace an existing prepared draft's mismatched assets casually: review and deliberately retire that unpublished preparation before selecting a new one. Identical assets are reused on retries; differing assets fail.

## Gates and assets

The native package matrix retains Linux x86-64, macOS ARM64 and Windows x86-64 and adds Linux ARM64. Every leg runs `cargo xtask verify`, `acceptance`, `completeness` and `package`. It executes the **packaged** binary, verifies its version and credential-free hello workflow, preserves its binary `SHA256SUMS`, license, README and shell completions, and generates a target-specific production CycloneDX SBOM.

The image matrix builds minimal and tooling on native Linux x86-64 and ARM64 runners. Each OCI archive carries the exact source/version labels and BuildKit provenance. Before assembly, each job transports its archive to a loopback-only disposable registry, pulls its platform, checks the non-root/read-only entrypoint and mounted workflow, and replays without network or credentials after removing mutable variable input. Fixed HIGH/CRITICAL vulnerabilities block both flavors. Platform-specific CycloneDX SBOMs and execution/scan records bind to the actual image config and manifest digests.

The assembly job verifies all inventories and hashes, then combines the original manifests and blobs into one OCI index per flavor. Provenance-only descriptors are validated separately from runnable platforms. It proves complete index push/pull and rerun reconciliation in a disposable registry. A GitHub artifact attestation binds `release-bundle.json` to the exact preparation workflow source; that manifest hashes the durable binary, image and evidence assets. Its Sigstore bundle is attached too, so publication does not depend on an unexpired Actions artifact.

Publication downloads the prepared release assets and verifies the attestation, checkout/tag/Cargo identity and completed preparation run before using registry credentials. This is distinct from claiming that every fresh Docker build is byte-for-byte reproducible.

The existing CI, container and supply-chain workflows remain required. They cover dependency policy, complete-history and tree secret scanning, workflow pins/lint, the production SBOM and existing runtime acceptance. Repository owners manage required status checks; these scripts do not change branch protection or organization settings.

## Image selection and tags

Both image flavors live in `docker.io/opensourceops/agentctl` and support `linux/amd64` and `linux/arm64`.

| Flavor | Immutable full-version tag | Mutable stable aliases |
| --- | --- | --- |
| Minimal | The release version | Its major.minor and `latest` |
| Tooling | The release version plus `-ci` | Its major.minor plus `-ci`, and `ci` |

There is no broad major-zero tag. Prereleases receive only full-version tags. Older releases cannot move a newer stable or minor alias backward. SemVer build metadata and release versions ending in `-ci` are rejected because they would introduce ambiguous identities or collide with the reserved tooling tags. Use the recorded **digest** for reproducible CI; mutable aliases are intended for exploration. See [container execution](CONTAINER.md) for the exact tools and mount contracts.

## Secrets and permissions

In the [framework repository](https://github.com/opensourceops/agentctl), open **Settings → Secrets and variables → Actions** and configure:

- Variable `DOCKERHUB_USERNAME`: the login identity authorized for the opensourceops namespace.
- Secret `DOCKERHUB_TOKEN`: an expiring token with push access to `opensourceops/agentctl`.

Only the publication job logs in. PR image validation and disposable registry tests need neither value. Workflow permissions default to read; only the manifest-attestation job receives OIDC/attestation write access, and only the draft-attachment job receives repository contents write access. The built-in `GITHUB_TOKEN` handles release assets. It never reaches a remediation model.

For the standalone demo, configure its own `OPENAI_API_KEY` and repository-scoped `GH_TOKEN` secrets, and `AGENTCTL_MODEL`/`AGENTCTL_IMAGE` variables. These are separate repositories; secrets are not inherited or copied. See the [remediation package](../examples/devops/21-container-remediation/README.md).

A `GITHUB_TOKEN` operation does not generally trigger downstream workflows. Explicit job dependencies connect preparation and attachment; the operator's manual Publish action starts image publication. The demo publisher uses its separately scoped token, whose access and repository policies determine whether generated PR checks start automatically. See GitHub's [token event behavior](https://docs.github.com/en/actions/concepts/security/github_token).

## Retry, partial failure and rollback

All production publications share one concurrency group. Existing full-version tags are inspected before writes; a different digest is a blocking mismatch. A failed/uncertain registry inspection is not treated as a missing tag. Both immutable flavor tags must be confirmed before stable aliases advance. Stable alias versions are checked again just before writing.

A timeout after copying OCI contents is reconciled by reading the remote tag. A rerun reuses matching content, continues missing content, and rejects mismatches. Partial alias updates are possible because a registry does not provide a transaction across tags; rerun reconciliation repairs them. This does not claim exactly-once delivery or protection from unrelated out-of-band writers. Registry credentials should be restricted to the reviewed publication path.

To retry after correcting registry access, dispatch **release-image-publication** on a trusted ref with the already published `release_tag`. It verifies the same durable assets and never rebuilds. Unavailable registries stop further writes. Do not overwrite an immutable version to conceal a failed retry.

Before the first publication, run **release-registry-preflight** on `main`. It uses the configured `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` to check the exact `opensourceops/agentctl` repository, then verifies anonymous public access. If the repository is absent, an operator can dispatch it with `create_if_missing: true` to create that public repository. Existing private repositories, authentication failures, and denied access stop the check without changing visibility or permissions. Docker Hub repository creation and metadata access may require token scopes beyond image push; configure these separately according to the [Docker Hub API](https://docs.docker.com/reference/api/hub/latest/) and [organization token permissions](https://docs.docker.com/security/access-tokens/organization-access-tokens/). The check does not prove image push access; actual digest promotion remains a separate gate.

Rollback means selecting a previously reviewed digest in the consuming system. It does not undo external deployments or rewrite released packages. Final production image publication and the demo's published-image smoke remain operator actions after implementation review.

## Local verification

Run the existing credential-free gates and the new release contracts:

```sh
cargo xtask verify
cargo xtask acceptance
cargo xtask completeness
cargo xtask package
cargo xtask acceptance-container
python3 -m unittest discover -s scripts/release -p 'test_*.py'
```

A protected build CA may be supplied through the existing container build secret mechanism. Keep TLS verification enabled. Hosted native execution and source-specific artifacts are required before claiming platform release coverage; local synthetic registry tests or workflow lint alone are insufficient.
