# agentctl

`agentctl` is a deterministic, declarative control plane for policy-constrained agentic automation. A versioned YAML workflow is compiled into a deterministically ordered task graph; deterministic actions and bounded model agents execute under one policy, effect ledger, SQLite history, and audit model.

Rust is the only production implementation. Node.js is not required to build, test, install, or run it. The former TypeScript runtime remains solely as an [archived compatibility reference](archive/TYPESCRIPT_REFERENCE.md).

## Quickstart

The repository pins Rust 1.88, the minimum supported version.

```console
cargo build --locked
cargo run -p agentctl-cli -- check examples/v1/hello.yaml
cargo run -p agentctl-cli -- plan examples/v1/hello.yaml
cargo run -p agentctl-cli -- run examples/v1/hello.yaml --db .agentctl/quickstart.db
```

The last command is credential-free and deterministic. Install from crates.io with:

```console
cargo install --locked agentctl-cli
```

For local development, use `cargo install --locked --path crates/agentctl-cli`.

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

Use `check` for strict syntax, references, templates, policy, and provider-capability validation. Use `plan` for deterministic order and predictability, `run --check --diff` for a non-mutating preview, `resume` after interruption, `replay` to reconstruct recorded results without effects, `retry` to rerun failed boundaries of an identical terminal workflow, `repair` to reuse compatible successful task boundaries with a corrected workflow, and `fork` when a broader fresh execution is intentional.

Terminal retry and selective repair are planned before execution:

```text
agentctl retry workflow.yaml SOURCE_RUN_ID --failed --plan
agentctl retry workflow.yaml SOURCE_RUN_ID --failed
agentctl repair repaired.workflow.yaml SOURCE_RUN_ID --from failed_task --plan
agentctl repair repaired.workflow.yaml SOURCE_RUN_ID --from failed_task
```

See [Retry a terminal workflow](docs/guides/TERMINAL_RETRY.md) and [Repair a failed workflow](docs/guides/repair-a-failed-workflow.md) for compatibility, lineage, state reconstruction, and uncertain-effect handling.
For retained pre-schema-5 history, use [Legacy run upgrade](docs/guides/LEGACY_RUN_UPGRADE.md). For ambiguous external outcomes, use [Effect reconciliation](docs/guides/EFFECT_RECONCILIATION.md).
For confidential workflow history, use [Sensitive-state encryption](docs/guides/SENSITIVE_STATE_ENCRYPTION.md).
For environment, mounted-file, and policy-gated process credentials, use [Secret references](docs/guides/SECRET_REFERENCES.md).

## Safety boundary

- Secrets are environment, mounted-file, or policy-gated process references, never inline values or CLI flags.
- Files, processes, providers, MCP servers, and A2A peers require explicit policy grants.
- Every non-pure operation is recorded before execution. A crash after an at-most-once effect starts is reported as uncertain and is never silently repeated.
- Model turns, output tokens, tool calls, retries, and time are bounded.
- Shell stdout/stderr capture is bounded, concurrently drained, and terminated/reaped on output, timeout, or cancellation limits.
- Check mode predicts deterministic actions; it does not claim to predict models or remote systems.
- The process policy is an allowlist, not an operating-system sandbox.

## Providers and protocols

CI uses the scripted fake provider. Native, mock-tested adapters cover OpenAI Responses, Azure OpenAI Responses, Anthropic Messages, and Google Gemini `generateContent`. MCP is pinned to `2025-11-25`; A2A is pinned to `1.0`. Live calls are always opt-in.

## Repository map

- `crates/agentctl-core`: DSL, compiler, templates, policy, state, effects, provider/tool contracts
- `crates/agentctl-runtime`: scheduler, actions, agent loop, resume/replay/repair/fork
- `crates/agentctl-store`: versioned SQLite persistence
- `crates/agentctl-providers`: native HTTP provider adapters
- `crates/agentctl-protocols`: MCP and A2A clients
- `crates/agentctl-observability`: audit-safe OpenTelemetry bridge
- `crates/agentctl-cli`: production CLI
- `xtask`: generated artifacts and canonical verification

Start with [Getting started](docs/guides/GETTING_STARTED.md), [Product](docs/PRODUCT.md), [Architecture](docs/ARCHITECTURE.md), [DSL](docs/DSL.md), [Operations](docs/OPERATIONS.md), [Container contract](docs/CONTAINER.md), [Troubleshooting](docs/guides/TROUBLESHOOTING.md), [Contributing](docs/CONTRIBUTING.md), [Limitations](docs/LIMITATIONS.md), [Security](docs/SECURITY.md), and the [generated CLI reference](docs/generated/CLI.md). Run the release-readiness layers with:

```console
cargo xtask verify
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask package
```

`cargo xtask acceptance-live-openai` is the explicit, credentialed live gate and is never part of normal CI. The production image uses `/config`, `/workspace`, `/state`, and `/artifacts` mounts, runs as non-root, and supports a read-only root filesystem.

Exit codes are stable: `0` success, `2` usage/validation, `3` policy or approval, `4` run failure, `5` persistence, `6` remote provider/protocol, and `130` cancellation. JSON output always uses the `agentctl.dev/cli/v1` envelope and never includes ANSI color.

Licensed under Apache-2.0.
