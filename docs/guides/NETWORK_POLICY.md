# Network policy

Agentctl treats outbound HTTP as an effect boundary, not as an implicit
capability. A required provider, MCP server, or A2A peer must pass network
preflight before the runtime creates a run record or effect.

## Public HTTPS policy

Use an exact host, HTTPS, and port 443 for a public provider:

```yaml
spec:
  policy:
    networkAllowlist: [api.openai.com]
    network:
      allowedSchemes: [https]
      allowedPorts: [443]
      connectTimeoutSeconds: 10
      maxResponseBytes: 4194304
      allowPrivate: false
      allowProxy: false
```

The host allowlist also accepts `*.example.com`. That rule matches
`api.example.com`, but it does not match `example.com` or
`api.example.com.evil`.

## Resolution and pinning

Preflight applies this sequence:

1. Parse the URL and reject credentials in the authority.
2. Require `http` or `https`, then apply `allowedSchemes`.
3. Match the exact or wildcard host grant.
4. Apply `allowedPorts` to the URL's explicit or scheme-default port.
5. Resolve a domain once within `connectTimeoutSeconds`.
6. Sort and deduplicate the complete IPv4 and IPv6 answer.
7. Reject the complete answer if it is empty or contains any forbidden
   address.
8. Pin every accepted address into the direct HTTP client.

The client therefore does not perform a second DNS lookup for the authorized
host. A mixed answer containing one public address and one private address
fails as a unit. IP-literal URLs use the same address checks without DNS.

With `allowPrivate: false`, agentctl denies private, loopback, link-local,
carrier-grade shared, documentation, benchmark, unspecified, multicast, and
reserved ranges. Set `allowPrivate: true` only for an intentional local or
internal peer:

```yaml
spec:
  policy:
    networkAllowlist: [127.0.0.1]
    network:
      allowedSchemes: [http]
      allowedPorts: [8765]
      allowPrivate: true
  mcpServers:
    local:
      url: http://127.0.0.1:8765/mcp
      protocolVersion: 2025-11-25
```

## Redirects, proxies, and Unix sockets

Provider and protocol clients never follow HTTP redirects. Authorize the final
endpoint directly instead of depending on a redirect chain.

Environment proxy discovery is disabled by default. `allowProxy: true`
explicitly opts into the HTTP client's environment proxy behavior. In that
mode the proxy controls the actual route and may perform destination
resolution, so the proxy becomes part of the trusted network boundary. For
hostile workflows, keep proxies disabled and enforce egress outside the
process as well.

Only HTTP and HTTPS URLs are accepted. Agentctl exposes no Unix-socket HTTP
configuration, and socket-specific URL schemes fail policy validation.

## TLS and custom roots

HTTPS uses rustls, hostname verification, and the installed platform roots.
TLS verification cannot be disabled.

An internal deployment may add a reviewed CA bundle through `customCa`. The
value is a protected secret reference and must contain one or more certificate
PEM blocks and no private keys or other PEM object types:

```yaml
spec:
  policy:
    environmentAllowlist: [CORP_CA_PEM]
    networkAllowlist: [agents.internal.example]
    network:
      allowedSchemes: [https]
      allowedPorts: [443]
      customCa: { env: CORP_CA_PEM }
```

A mounted file or policy-gated process secret reference is also valid. The
resolved bundle remains in zeroizing adapter memory and is not written to the
workflow, database, effects, audit, traces, or CLI output. Invalid or empty
bundles fail before dispatch.

## Time and size bounds

`connectTimeoutSeconds` bounds DNS resolution and TCP setup. Provider task
timeouts and MCP/A2A operation timeouts bound the wider operation.

`maxResponseBytes` is an upper bound that composes with lower adapter limits.
Provider JSON is capped at 4 MiB, OpenAI streaming at 8 MiB, and MCP/A2A
responses at 4 MiB even if the policy value is higher. Set a smaller policy
value when the expected contract is narrower. Oversized success and error
responses fail without entering task state.

Network policy denial exits with code `3`. DNS failure or DNS timeout exits
with code `6`. Cancellation exits with code `130`. These preflight failures do
not create a runtime database when it does not already exist.

This policy governs runtime provider, MCP, and A2A clients. Remote pack
acquisition is an operator configuration action with its own immutable-source,
digest, redirect, and size controls. Use `--locked --offline` when an untrusted
workflow must not initiate pack acquisition.

Policy checks are defense in depth, not an OS sandbox. Containers, VMs,
least-privilege identities, and platform egress rules remain the stronger
boundary for untrusted executors.
