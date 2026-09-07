# ADR 0007: Generic OCI step contract

Status: accepted

## Decision

One minimal OCI image serves Docker, Kubernetes, and container-step CI systems. It contains only the Rust CLI on a maintained distroless base, runs as UID/GID 65532, and supports a read-only root filesystem. The stable mount contract is `/config` (read-only configuration), `/workspace` (usually read-only source), `/state` (SQLite durability), and `/artifacts` (declared output).

The image entrypoint is `agentctl`. Callers supply ordinary CLI arguments and one final JSON result is written to stdout. Credentials are injected as environment variables or platform secret mounts, never ordinary CLI values. `/state` must persist between execution, inspect, approval, resume, and replay invocations.

## Consequences

No vendor-specific plugin API is required. Platforms without direct entrypoint/argument support can invoke `docker run` from their normal shell step. Distroless reduces runtime surface but deliberately has no shell; debugging uses the public CLI or a separate diagnostic image, not mutation of production images.

## Current implementation clarification

The original minimal image decision remains the default build target. A separate tooling target adds a shell, Python and Git for reviewed process adapters and shell-based CI Run steps, while retaining the non-root identity and agentctl entrypoint. It grants no Docker socket, publisher token or workflow process authority automatically.

For captured `instructionsFile` and `varsFiles`, configuration must be within the selected workspace's canonical read boundary. Place reviewed workflow, pack and external configuration together beneath `/workspace/config`; a standalone `/config` mount outside `--workspace /workspace` does not authorize those source reads. Preserve separate writable `/state` and explicitly policy-granted `/artifacts` mounts. The [current container guide](../CONTAINER.md) supersedes the original path illustration for file-backed workflows and documents image selection and host-directory ownership.
