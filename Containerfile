# syntax=docker/dockerfile:1.7
FROM rust:1.88.0-bookworm AS build
ENV RUSTUP_TOOLCHAIN=1.88.0
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
    cargo build --release --locked -p agentctl-cli

FROM gcr.io/distroless/cc-debian12:nonroot
ARG AGENTCTL_VERSION=0.2.0
LABEL org.opencontainers.image.title="agentctl" \
      org.opencontainers.image.description="Deterministic control plane for policy-constrained agentic automation" \
      org.opencontainers.image.version="${AGENTCTL_VERSION}" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.source="https://github.com/opensourceops/agentctl"
COPY --from=build --chown=nonroot:nonroot /source/target/release/agentctl /usr/local/bin/agentctl
USER nonroot:nonroot
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/agentctl"]
