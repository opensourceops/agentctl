# Install the agentctl candidate

The Rust CLI and crates are **pre-1.0**. `agentctl.dev/v1` identifies the workflow document format; it does not mean the product is a stable 1.0 release. These tutorials use candidate features from the paired source revision. A previously published crate or container may not contain them.

## Install matching source

You need Git, Rust 1.88.0, and Cargo. Installation downloads build dependencies and needs no provider key. The documentation site renders the exact source revision and copyable command here:

<!-- agentctl-candidate-install -->

When reading this file in a source checkout, install that checkout directly:

```sh
cargo install --locked --path crates/agentctl-cli
agentctl version
```

Run this command from the repository root. Cargo installs the executable into its binary directory, normally `~/.cargo/bin`; include that directory in your `PATH`. Keep the checkout's `git rev-parse HEAD` value with your installation evidence. The version string alone does not identify an unmerged candidate commit.

For an isolated installation, pass `--root /tmp/agentctl-candidate` and use `/tmp/agentctl-candidate/bin/agentctl`. On Windows, use a writable directory of your choice and its `bin/agentctl.exe`.

## Verify without a source checkout

Continue with [your first credential-free workflow](GETTING_STARTED.md). That page includes the entire YAML document. Once the binary is installed, no repository checkout, Python dependency, container engine, or provider key is needed for that first run.

The DevOps cookbook adds Python and case-specific tools. Download a complete matching example package from its tutorial; copying a workflow alone omits the helper, schemas, and input files it requires.

## Build a local package or image

From the reviewed source checkout:

```sh
cargo xtask package
```

The local `dist/` output includes the release binary, completions, license, README, and SHA-256 manifest. Building this package does not publish a release.

For container operation, build the same checkout with Docker or Podman:

```sh
docker build --tag agentctl:candidate --file Containerfile .
docker run --rm agentctl:candidate version --output json --color never
```

The image runs as UID/GID 65532 with `agentctl` as its entrypoint. Review the [container contract](../CONTAINER.md) before mounting a workspace or durable database. Record both the source commit and resulting image digest.

## Published versions and upgrades

This guide does not establish that a published crate or registry image contains the candidate changes. Use a published release only with documentation and artifacts verified for that release. Keep the source revision, binary checksum, and workflow assets together.

Before an upgrade, review [compatibility](../COMPATIBILITY.md) and [limitations](../LIMITATIONS.md), back up SQLite state using the [state guide](../reference/DATABASE.md), then run `check` and `plan` with the new binary. `agentctl update` explains installation paths; it does not replace your binary.
