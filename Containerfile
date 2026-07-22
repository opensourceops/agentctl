# syntax=docker/dockerfile:1.7
FROM rust:1.88.0-bookworm AS build
WORKDIR /source

COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
COPY xtask ./xtask
RUN cargo build --release --locked -p agentctl

FROM gcr.io/distroless/cc-debian12:nonroot
ARG AGENTCTL_VERSION=0.2.0
LABEL org.opencontainers.image.title="agentctl" \
      org.opencontainers.image.description="Deterministic control plane for policy-constrained agentic automation" \
      org.opencontainers.image.version="${AGENTCTL_VERSION}" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.source="https://github.com/ompragash/agentctl"
COPY --from=build --chown=nonroot:nonroot /source/target/release/agentctl /usr/local/bin/agentctl
USER nonroot:nonroot
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/agentctl"]
