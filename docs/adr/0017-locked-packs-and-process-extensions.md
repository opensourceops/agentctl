# ADR 0017: Locked packs and process extensions

- Status: accepted
- Date: 2026-07-27

## Context

Direct local pack digests did not support transitive reuse, immutable remote
sources, offline execution, publisher policy, or a safe executable extension
contract. An in-process native ABI would place third-party code inside the Rust
safety and durability boundary.

## Decision

Packs resolve from contained local paths, full-commit Git URLs, or immutable
digest-pinned tar-gzip archives. Every requirement names one source and semantic
constraint. `agentctl.pack.lock` v1 stores a canonical concrete graph, source,
compatibility, content digest, dependency edges, signature metadata, and trust
result. Locked and offline modes are explicit.

Optional publisher verification uses standard Sigstore bundles with exact
identity and issuer policy. Unsigned behavior is deny, warn, or allow. A pack
that declares process execution cannot load without verified trust or an
explicit unsigned-process review acknowledgement.

There is no native dynamic-library ABI. Executable local extensions use
`agentctl.dev/process-extension/v1`: a non-mutating handshake validates exact
version, JSON Schemas, and capabilities before a separately bounded invocation
receives a durable effect ID and input. MCP remains the remote tool extension.

## Consequences

Resolution is reproducible without a hosted registry. Git branches, tags,
redirecting archives, archive links, implicit dependency discovery, and
credential-bearing URLs are rejected. Sigstore verification depends on the
trust root embedded in the installed agentctl version.

Process extensions remain operating-system processes, not sandboxes. Policy,
direct argv, cleared environment, output/time bounds, cancellation, effect
uncertainty, and pack trust reduce risk but do not isolate hostile code.
Containers, VMs, and platform sandboxes remain the stronger boundary.
