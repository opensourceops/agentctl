# ADR 0007: Generic OCI step contract

Status: accepted

## Decision

One minimal OCI image serves Docker, Kubernetes, and container-step CI systems. It contains only the Rust CLI on a maintained distroless base, runs as UID/GID 65532, and supports a read-only root filesystem. The stable mount contract is `/config` (read-only configuration), `/workspace` (usually read-only source), `/state` (SQLite durability), and `/artifacts` (declared output).

The image entrypoint is `agentctl`. Callers supply ordinary CLI arguments and one final JSON result is written to stdout. Credentials are injected as environment variables or platform secret mounts, never ordinary CLI values. `/state` must persist between execution, inspect, approval, resume, and replay invocations.

## Consequences

No vendor-specific plugin API is required. Platforms without direct entrypoint/argument support can invoke `docker run` from their normal shell step. Distroless reduces runtime surface but deliberately has no shell; debugging uses the public CLI or a separate diagnostic image, not mutation of production images.
