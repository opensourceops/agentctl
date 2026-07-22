# agentctl

`agentctl` is a deterministic, declarative control plane for policy-constrained agentic automation. A versioned YAML workflow is compiled into a deterministically ordered task graph; deterministic actions and bounded model agents execute under one policy, effect ledger, SQLite history, and audit model.

Rust is the only production implementation. Node.js is not required to build, test, install, or run it. The former TypeScript runtime remains solely as an [archived compatibility reference](archive/TYPESCRIPT_REFERENCE.md).

## Quickstart

The repository pins Rust 1.88, the minimum supported version.

```console
cargo build --locked
cargo run -p agentctl -- check examples/v1/hello.yaml
cargo run -p agentctl -- plan examples/v1/hello.yaml
cargo run -p agentctl -- run examples/v1/hello.yaml --db .agentctl/quickstart.db
```

The last command is credential-free and deterministic. Install locally with:

```console
cargo install --locked --path crates/agentctl-cli
```

For a tool-using credential-free journey, copy `examples/acceptance/mock-tool` to a clean directory and run its `workflow.yaml`. The repository acceptance suite executes that exact journey outside the source tree.

## Workflow

```yaml
apiVersion: agentctl.dev/v1alpha1
kind: Workflow
metadata:
  name: hello
spec:
  actions:
    greeting:
      kind: builtin.assign
  tasks:
    - id: hello
      uses: action:greeting
      with:
        message: hello from agentctl
```

Use `check` for strict syntax, references, templates, policy, and provider-capability validation. Use `plan` for deterministic order and predictability, `run --check --diff` for a non-mutating preview, `resume` after interruption, `replay` to reconstruct recorded results without effects, and `fork` when fresh effects are intentional.

## Safety boundary

- Secrets are environment references, never inline values or CLI flags.
- Files, processes, providers, MCP servers, and A2A peers require explicit policy grants.
- Every non-pure operation is recorded before execution. A crash after an at-most-once effect starts is reported as uncertain and is never silently repeated.
- Model turns, output tokens, tool calls, retries, and time are bounded.
- Check mode predicts deterministic actions; it does not claim to predict models or remote systems.
- The process policy is an allowlist, not an operating-system sandbox.

## Providers and protocols

CI uses the scripted fake provider. Native, mock-tested adapters cover OpenAI Responses, Azure OpenAI Responses, Anthropic Messages, and Google Gemini `generateContent`. MCP is pinned to `2025-11-25`; A2A is pinned to `1.0`. Live calls are always opt-in.

## Repository map

- `crates/agentctl-core`: DSL, compiler, templates, policy, state, effects, provider/tool contracts
- `crates/agentctl-runtime`: scheduler, actions, agent loop, resume/replay/fork
- `crates/agentctl-store`: versioned SQLite persistence
- `crates/agentctl-providers`: native HTTP provider adapters
- `crates/agentctl-protocols`: MCP and A2A clients
- `crates/agentctl-observability`: audit-safe OpenTelemetry bridge
- `crates/agentctl-cli`: production CLI
- `xtask`: generated artifacts and canonical verification

Start with [Product](docs/PRODUCT.md), [Architecture](docs/ARCHITECTURE.md), [DSL](docs/DSL.md), [Operations](docs/OPERATIONS.md), [Container contract](docs/CONTAINER.md), [Limitations](docs/LIMITATIONS.md), [Security](docs/SECURITY.md), and the [generated CLI reference](docs/generated/CLI.md). Run the release-readiness layers with:

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask package
```

`cargo xtask acceptance-live-openai` is the explicit, credentialed live gate and is never part of normal CI. The production image uses `/config`, `/workspace`, `/state`, and `/artifacts` mounts, runs as non-root, and supports a read-only root filesystem.

Exit codes are stable: `0` success, `2` usage/validation, `3` policy or approval, `4` run failure, `5` persistence, `6` remote provider/protocol, and `130` cancellation. JSON output always uses the `agentctl.dev/cli/v1` envelope and never includes ANSI color.

Licensed under Apache-2.0.
