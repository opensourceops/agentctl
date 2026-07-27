# ADR 0019: Preflighted and pinned network egress

- Status: accepted
- Date: 2026-07-27

## Context

Host allowlists and disabled redirects did not constrain effective ports,
resolved addresses, private networks, environment proxies, custom roots, or
response size. Authorizing a hostname and allowing the HTTP client to resolve
it later also left a DNS rebinding gap between policy evaluation and
connection.

Policy checks cannot become an OS network sandbox, but the runtime can make its
own provider and protocol clients fail closed and deterministic.

## Decision

Required provider, MCP, and A2A adapters perform network preflight before run
persistence. Preflight authorizes the HTTP(S) scheme, exact or wildcard host,
and effective port. Domain targets resolve once within a bound. Every returned
IPv4 and IPv6 address must pass classification, and the complete accepted set
is pinned into the direct HTTP client.

Private and reserved addresses are denied by default. Workflows may explicitly
allow them for intentional local or internal peers. Redirects and Unix-socket
transports remain unsupported. Environment proxies are disabled by default;
explicit proxy opt-in delegates routing and name resolution to that trusted
proxy.

TLS uses rustls and normal platform roots. A workflow may add a certificate-only
PEM bundle through the existing protected secret-reference boundary. DNS and
connect setup have a policy bound. Response bytes are capped by the smaller of
the policy value and each adapter's hard safety limit.

## Consequences

Public HTTP(S) workflows keep their existing scheme defaults, but local and
private endpoints now require `allowPrivate: true`. An empty or mixed
public/private DNS answer fails as a unit. Direct connections cannot perform a
second name lookup after authorization.

Unused providers are not resolved. A required adapter that fails policy or
address classification returns policy exit code `3` before creating a new
runtime database. DNS operational failures use remote exit code `6`.

Explicit proxy mode has a narrower guarantee because the proxy owns the actual
route. External egress isolation remains the stronger boundary for hostile
workflows, and policy documentation must not describe these checks as a
sandbox.
