# Install agentctl

Choose a published CLI package, a container image, or an exact-source build. Use the
same source and artifacts when reproducing a workflow. `agentctl.dev/v1` identifies
the workflow document API; executable, provider and durable-state compatibility
are documented separately.

## CLI release downloads

Open the [latest release](https://github.com/opensourceops/agentctl/releases/latest),
select the archive for your operating system and architecture, and verify its
checksum against the release's checksum manifest before extracting it. The
[release list](https://github.com/opensourceops/agentctl/releases) retains earlier
artifacts and their source records. Put the extracted `agentctl` executable on
your `PATH`, then run:

```sh
agentctl version --output json --color never
```

Release preparation attaches complete binary assets before publication. If the
release or required asset is not available yet, use the source-build path below.
These instructions do not assume crates.io publication or `cargo install agentctl`.

## Container image

After the maintainer publishes the image, use the Docker Hub repository
`opensourceops/agentctl`. For quick exploration:

```sh
docker run --rm docker.io/opensourceops/agentctl:latest version --output json --color never
```

`latest` is mutable. For reproducible CI, set `AGENTCTL_IMAGE` to the exact
`docker.io/opensourceops/agentctl@sha256:...` reference recorded in the release
summary and use that value in your pipeline. The [container guide](../CONTAINER.md)
explains minimal and tooling images, entrypoints, mounts and a complete workflow.
A source build validates unreleased changes without assuming the Docker Hub tags
already exist.

## Install matching source

You need Git, Rust 1.88.0, and Cargo. Installation downloads build dependencies and
needs no provider key. The documentation site renders the exact source revision
and copyable command here:

<!-- agentctl-source-install -->

When reading this file in a source checkout, install that checkout directly:

```sh
cargo install --locked --path crates/agentctl-cli
agentctl version
```

Run from the repository root. Cargo installs the executable into its binary
directory, normally `~/.cargo/bin`; include that directory in your `PATH`. Retain
`git rev-parse HEAD` with the installed binary checksum. For an isolated
installation, pass `--root /tmp/agentctl-install` and use
`/tmp/agentctl-install/bin/agentctl`. On Windows, choose a writable directory and
use its `bin/agentctl.exe`.

## First workflow

Continue with [your first credential-free workflow](GETTING_STARTED.md). That
page includes the entire YAML document. Once the binary is installed, the first
run needs no source checkout, Python dependency, container engine, or provider key.

The DevOps cookbook adds Python and case-specific tools. Download a complete
matching example package from its tutorial; copying a workflow alone omits the
helper, schemas, and input files it requires.

## Build a local package or image

From the reviewed source checkout:

```sh
cargo xtask package
docker build --tag agentctl:local --file Containerfile .
docker run --rm agentctl:local version --output json --color never
```

The local `dist/` package includes the binary, completions, license, README, and
SHA-256 manifest. Building a package or local image does not publish it. The
minimal image uses `agentctl` as its entrypoint. Review the [container
contract](../CONTAINER.md) before mounting a workspace or durable database and
record the source commit and resulting image identity.

## Upgrades

Review [compatibility](../COMPATIBILITY.md) and [limitations](../LIMITATIONS.md),
back up SQLite state using the [state guide](../reference/DATABASE.md), then run
`check` and `plan` with the new executable. `agentctl update` explains installation
paths; it does not replace your binary.
