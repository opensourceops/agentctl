# ADR 0020: Explicit process and container isolation

- Status: accepted
- Date: 2026-07-27

## Context

Direct arguments, cleared environments, allowlists, output limits, timeouts,
and process-tree termination constrain process invocation, but they do not
remove the host identity's filesystem, network, IPC, or signaling authority.
Describing those checks as sandboxing would give operators a false security
guarantee.

Platform-specific native sandboxes have materially different availability and
semantics across Linux, macOS, and Windows. A weak emulation would be less
honest than an explicit portable boundary.

## Decision

Every shell or process-extension action has an inspectable isolation mode.
Existing actions default to `process`, which is documented as host execution
and not a sandbox. `container` is the portable stronger mode.

Container mode requires a locally available digest-pinned image and Docker or
Podman. The runtime disables pulls and networking, uses a read-only root and
workspace mount, runs non-root, drops capabilities, enables
`no-new-privileges`, bounds memory/CPU/PIDs/output/time, and passes only
declared environment values. The payload command is the fixed entrypoint, so
payload arguments cannot become engine options.

The selected engine and image are inspected before an effect request. Missing
or unavailable backends fail closed. A timeout, cancellation, or output-limit
failure terminates the engine client and issues a forced removal for the named
container. Failure to confirm cleanup is surfaced rather than ignored; effect
certainty still follows the normal action or extension dispatch boundary.

The compiler records process isolation requirements in the plan. No Linux
namespace, bubblewrap, macOS sandbox-profile, Windows restricted-token, or job
object isolation backend is claimed.

## Consequences

Existing workflows retain host-process behavior, now visible as `process`.
Users who need a stronger boundary must prepare a content-addressed image and
request `container`; agentctl never silently falls back.

Container mode intentionally has no network and only a read-only workspace
mount. Workflows that need a different mount or network contract must run the
whole agentctl OCI step in an externally configured worker boundary instead of
weakening an individual action.

Containers remain dependent on a trusted engine and host kernel or VM.
Explicit secrets and readable workspace content remain visible to the selected
image. External workload identity, image admission, VM isolation, and platform
egress controls remain appropriate for hostile code.
