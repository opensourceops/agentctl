# Packs

A pack is reviewed reusable workflow content. It is not an in-process native
plugin. The manifest API is `agentctl.dev/pack/v1alpha1` and uses a fully
qualified dotted name, semantic version, agentctl compatibility constraint,
optional dependencies, and exported actions, agents, tools, and sub-workflows.

## Sources and resolution

Workflow roots and transitive dependencies use an exact source:

```yaml
packs:
  - name: example.utility
    version: "^1.0"
    source:
      path: packs/example.pack.yaml
```

Supported sources are:

- a contained local `path`;
- a Git `https` URL with a full 40-character `rev` and contained `manifest`;
- an immutable `https` tar-gzip archive URL with SHA-256 `integrity` and a
  contained `manifest`.

Loopback HTTP and contained `file:` Git URLs exist only for deterministic local
fixtures. URLs with credentials, query parameters, or fragments are rejected.
Git branches and tags are not accepted as revisions. Archive redirects,
symlinks, hardlinks, special files, path escapes, more than 1,024 entries,
compressed content over 16 MiB, and expanded content over 64 MiB are rejected.

Each dependency names its source and semantic constraint:

```yaml
dependencies:
  example.base:
    version: "^2.1"
    source:
      git: https://github.com/example/base-pack.git
      rev: 0123456789abcdef0123456789abcdef01234567
      manifest: agentctl.pack.yaml
```

Resolution is deterministic because every requirement identifies one immutable
candidate. Conflicting versions or sources, duplicate identities, missing
dependencies, and cycles fail with validation exit `2`. There is no hosted
registry and no implicit search order.

## Lockfile workflow

Generate and commit `agentctl.pack.lock` beside the workflow:

```console
agentctl packs lock workflow.yaml
agentctl packs verify-lock workflow.yaml --locked
agentctl packs update workflow.yaml
agentctl packs update workflow.yaml --pack example.utility
```

The lock API is `agentctl.dev/pack-lock/v1`. It records agentctl compatibility,
pack identity and concrete version, exact source, manifest digest, dependency
edges, signature metadata, and trust result. Entries are sorted by pack name.
An update resolves the complete graph; `--pack` validates and identifies the
requested root while preserving graph-wide consistency.

Execution uses the lock whenever it exists. `--locked` requires it and rejects
workflow, source, graph, digest, compatibility, signature, trust, or unreachable
entry drift. `--offline` permits local paths and requires Git/archive cache
hits. Legacy exact `path` plus `integrity` references remain readable without a
lock and emit a migration warning.

## Integrity and trust

SHA-256 binds the lock to exact bytes. It proves sameness, not publisher
identity. Optional keyless publisher verification uses the standard Sigstore
bundle format and the embedded Sigstore public-good trust root:

```yaml
packTrust:
  unsigned: deny
  identities:
    - identity: https://github.com/example/repository/.github/workflows/release.yml@refs/tags/v1.2.0
      issuer: https://token.actions.githubusercontent.com
packs:
  - name: example.utility
    version: "=1.2.0"
    source:
      path: packs/example.pack.yaml
    signature:
      bundle: packs/example.pack.sigstore.json
      identity: https://github.com/example/repository/.github/workflows/release.yml@refs/tags/v1.2.0
      issuer: https://token.actions.githubusercontent.com
```

Verification checks the artifact signature, certificate chain, identity,
issuer, transparency-log proof, signed timestamp, and locked bundle digest.
The bundle is sufficient for offline cryptographic verification, subject to the
freshness of the trust root embedded in the installed agentctl version. See the
[Sigstore bundle model](https://docs.sigstore.dev/about/bundle/) and
[Cosign blob verification](https://docs.sigstore.dev/cosign/verifying/verify/).

`packTrust.unsigned` is `deny`, `warn`, or `allow`, with `warn` as the default.
Unsigned process-capable packs are always blocked unless
`allowUnsignedProcess: true` is also explicit. That exception is a review
acknowledgement, not proof of publisher identity or process isolation.

Manifest policy defaults remain inspectable metadata and never weaken the
invoking workflow policy. Exported definitions are qualified as
`<pack-name>.<item-name>` before compilation.

## Verification evidence

Unit fixtures cover semantic constraints, deterministic ordering, conflicts,
cycles, path containment, tamper, locked drift, pinned Git cache reuse,
offline misses, bounded archives, archive links, valid and invalid Sigstore
bundles, identity policy, unsigned process denial, and dependency reachability.
Packaged acceptance scenario 42 verifies the checked-in transitive example,
locks an extension pack, proves that its process cannot start before explicit
trust authorization, executes it once, and replays without another invocation.
