# syntax=docker/dockerfile:1.7
ARG AGENTCTL_VERSION=0.4.0
ARG AGENTCTL_REVISION=unknown

FROM rust:1.88.0-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS build
ARG AGENTCTL_VERSION
ENV RUSTUP_TOOLCHAIN=1.88.0 CARGO_BUILD_JOBS=2
WORKDIR /source

COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
COPY xtask ./xtask
RUN --mount=type=secret,id=agentctl_ca,required=false \
    --mount=type=tmpfs,target=/tmp/agentctl-ca \
    set -eu; \
    if [ -s /run/secrets/agentctl_ca ]; then \
      cat /etc/ssl/certs/ca-certificates.crt /run/secrets/agentctl_ca \
        > /tmp/agentctl-ca/combined-ca.pem; \
      export CARGO_HTTP_CAINFO=/tmp/agentctl-ca/combined-ca.pem; \
      export SSL_CERT_FILE=/tmp/agentctl-ca/combined-ca.pem; \
    fi; \
    cargo build --release --locked -p agentctl-cli; \
    test "$(target/release/agentctl --version)" = "agentctl ${AGENTCTL_VERSION}"

# Explicit CI target. The model never receives a Docker socket or publisher token.
FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS tooling
ARG AGENTCTL_VERSION
ARG AGENTCTL_REVISION
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      ca-certificates git python3 python3-packaging python3-ruamel.yaml \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir /workspace \
    && chown 65532:65532 /workspace
LABEL org.opencontainers.image.title="agentctl" \
      org.opencontainers.image.description="agentctl with bounded CI helper prerequisites" \
      org.opencontainers.image.version="${AGENTCTL_VERSION}" \
      org.opencontainers.image.revision="${AGENTCTL_REVISION}" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.source="https://github.com/opensourceops/agentctl" \
      dev.agentctl.variant="tooling"
COPY --from=build --chown=65532:65532 /source/target/release/agentctl /usr/local/bin/agentctl
COPY scripts/release_live_budget.py scripts/live_command.py /opt/agentctl/release-budget/scripts/
COPY examples/devops/live_budget.py /opt/agentctl/release-budget/examples/devops/live_budget.py
USER 65532:65532
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/agentctl"]

# Keep the minimal runtime last so an ordinary build preserves the default image.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS minimal
ARG AGENTCTL_VERSION
ARG AGENTCTL_REVISION
LABEL org.opencontainers.image.title="agentctl" \
      org.opencontainers.image.description="Deterministic control plane for policy-constrained agentic automation" \
      org.opencontainers.image.version="${AGENTCTL_VERSION}" \
      org.opencontainers.image.revision="${AGENTCTL_REVISION}" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.source="https://github.com/opensourceops/agentctl" \
      dev.agentctl.variant="minimal"
COPY --from=build --chown=nonroot:nonroot /source/target/release/agentctl /usr/local/bin/agentctl
USER nonroot:nonroot
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/agentctl"]
