# Install agentctl

This guide installs the current `v1alpha1` release-candidate source. There is no published registry package or guaranteed downloadable binary in this checkout, so use a reviewed source build or build the OCI image yourself.

## Prerequisites

- Git
- Rust 1.88, as pinned by `rust-toolchain.toml`
- A supported local checkout
- Optional: Docker or Podman for the container path

The repository configures hosted checks for Linux x64, macOS arm64, and Windows x64. Those workflows have not executed on the current remote candidate, so treat cross-platform support as configured and pending hosted evidence.

## Install from source

From the `agentctl` repository root, run:

```text
cargo install --locked --path crates/agentctl-cli
agentctl version
```

The install compiles the Rust CLI and writes it to Cargo's binary directory. It makes dependency network requests during the build, writes no runtime database, and needs no provider credential.

If you only want a repository-local binary, use:

```text
cargo build --locked -p agentctl
./target/debug/agentctl version
```

## Build a release binary

From the repository root:

```text
cargo build --release --locked -p agentctl
./target/release/agentctl version
```

`cargo xtask package` also produces the binary, shell completions, license, README, and SHA-256 manifest beneath `dist/`. The package is local build output, not a published release.

## Build the container image

From the repository root, with Docker:

```text
docker build --tag agentctl:local --file Containerfile .
docker run --rm agentctl:local version --output json --color never
```

The build downloads Rust dependencies. The version command makes no provider call and writes no state. The resulting image runs as UID/GID 65532 with `agentctl` as its entrypoint. Read the [container contract](../CONTAINER.md) before executing a workflow.

## Verify the installation

From the repository root:

```text
agentctl check examples/v1/hello.yaml
agentctl plan examples/v1/hello.yaml
```

Expected evidence includes `valid: hello`, task order `greet`, and `FullyPredictable`. These commands do not create a runtime database.

## Upgrade safely

The workflow API is `agentctl.dev/v1alpha1`. Pin the CLI or image version, read [compatibility](../COMPATIBILITY.md) and [limitations](../LIMITATIONS.md), back up the SQLite database with its WAL files, then validate workflows before replacing a binary. `agentctl update` explains supported update paths but does not modify the installation.

## Next step

Continue with [Getting started](GETTING_STARTED.md).
