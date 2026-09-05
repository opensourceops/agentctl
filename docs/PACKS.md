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
- a cached Git source with a full 40-character `rev` and contained `manifest`;
- an immutable `https` tar-gzip archive URL with SHA-256 `integrity` and a
  contained `manifest`.

Fresh archive downloads use the invoking workflow's network policy, including
scheme, host, effective port, and every resolved IP address. The client pins
approved DNS answers, uses the policy's custom CA and connect/response limits,
disables redirects, and ignores environment proxies unless `allowProxy: true`
is explicit. A verified cached archive needs no new network grant.

Fresh remote HTTPS Git acquisition is rejected because the Git transport does
not enforce the same DNS-pinning contract. Use an immutable digest-pinned
HTTPS archive or a previously populated exact-commit cache instead. Contained
`file:` Git sources and existing pinned Git caches remain supported; inherited
Git configuration and protocol rewrites are disabled. `--offline` requires a
cache hit for remote sources.

Loopback HTTP archives and contained `file:` Git URLs support deterministic
local fixtures. URLs with credentials, query parameters, or fragments are rejected.
Git branches and tags are not accepted as revisions. Archive redirects,
symlinks, hardlinks, special files, path escapes, more than 1,024 entries,
compressed content over 16 MiB, and expanded content over 64 MiB are rejected.

Each dependency names its source and semantic constraint. For example:

```yaml
dependencies:
  example.base:
    version: "^2.1"
    source:
      path: base/agentctl.pack.yaml
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

## Configuration assets

Pack instruction and variable files must appear in the manifest's `files` map.
Each key is a normalized portable relative path and each value is the exact
`sha256:` digest of that file. These assets are bounded text configuration:
regular UTF-8 files of at most 1 MiB each, at most 256 declarations, and at most
16 MiB in total. They do not provide a general binary-file packaging mechanism.

`agentctl packs verify PACK_FILE --integrity sha256:MANIFEST_DIGEST` validates
every declared asset, including
unused entries, for containment, file type, size, encoding, and content digest.
The lock and any publisher signature bind the manifest declarations. Workflow
source capture verifies bytes used by `instructionsFile` and `varsFiles` and
also applies the invoking workflow's workspace/read policy. A trusted pack
cannot expand that policy. Resolve relative source paths from the declaring
pack manifest; update its digests and the reviewed lock after changing assets.
See [Variables and instruction files](VARIABLES.md).

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
offline misses, network-policy denials before acquisition, bounded archives,
archive links, declared asset tampering, valid and invalid Sigstore
bundles, identity policy, unsigned process denial, and dependency reachability.
Packaged acceptance scenario 42 verifies the checked-in transitive example,
locks an extension pack, proves that its process cannot start before explicit
trust authorization, executes it once, and replays without another invocation.
