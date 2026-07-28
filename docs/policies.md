# Policies and approvals

Policy is evaluated by the runtime, never by a model. A policy defines a
canonical workspace root, writable roots, allowed environment names, secret
file roots, ordinary and secret-helper process basenames, network host
patterns, providers, tool allow/deny lists, approval mode, and non-interactive
behavior.

Read paths must canonicalize under the workspace. Write paths canonicalize the
nearest existing parent and must remain under a writable root. Secret files
must be existing regular files canonically contained by `secretFileRoots`.
Parent traversal and symlink escape fail. Network rules match an exact hostname
or `*.suffix` subdomains; suffix lookalikes and the wildcard apex do not match.
Before a required provider, MCP, or A2A adapter is created, agentctl validates
the scheme and effective port, resolves the destination once, checks every
returned IPv4 and IPv6 address, and pins the accepted answer into the HTTP
client. Private, loopback, link-local, shared, documentation, benchmark,
unspecified, multicast, and reserved addresses fail unless `allowPrivate` is
explicitly enabled. HTTP redirects and Unix-socket transports are disabled.
Environment proxy discovery is disabled unless `allowProxy` is explicit.
TLS uses rustls and the platform roots; an optional protected `customCa`
reference may add a certificate-only PEM bundle. DNS/connect time and response
bytes are bounded. See [Network policy](guides/NETWORK_POLICY.md).

Process allowlisting checks the executable basename. Process actions then use
an explicit isolation mode. The default `process` mode launches direct host
arguments with a cleared environment and is not a sandbox. `container`
requires a local digest-pinned image and available Docker or Podman backend;
it never falls back to the host, pulls an image, or enables network access.
The compiled plan exposes the selected mode and resource limits. Secret
helpers use their separate `secretProcessAllowlist` and stricter 60-second,
64-KiB maximums as host processes. See [Process
isolation](guides/PROCESS_ISOLATION.md) and [Secret
references](guides/SECRET_REFERENCES.md).

Tool visibility, tool/capability authorization, resource checks, effect risk, and approval are distinct decisions. `never`, `mutations`, `high_risk`, and `always` are available approval modes. A tool may say `never`, `policy`, or `always`. The default non-interactive behavior is a durable pause and exit code `3`; explicit `deny_approval` and `fail` modes fail closed. Non-interactive execution never prompts or auto-approves.

An approval stores the run/trace/task/agent, tool, capability, risk, redacted input, expected effect, reason, and resolution actor/reason. The associated task waits durably. Use `approvals list`, `approve`, or `reject`, then `resume`. Resolution and effect status are auditable.

Provider, MCP, A2A, filesystem, process, and environment allowlists are
necessary controls, not a containment boundary. Use `isolation: container` or
an externally managed container/VM boundary for untrusted executors.
