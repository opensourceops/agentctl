# Developer guide

## Repository layout

The production implementation is a Rust workspace. The remaining top-level TypeScript source is an archived compatibility reference and has no production package entrypoint.

| Crate | Responsibility |
| --- | --- |
| `agentctl-core` | strict DSL, migration, compiler, templates, state, effects, policy, provider and tool contracts |
| `agentctl-store` | versioned SQLite persistence, migrations, checkpoints, approvals, audit, trace, sessions, tool calls, memory |
| `agentctl-runtime` | sequential scheduler, actions, bounded agent loop, resume, replay, fork, cancellation |
| `agentctl-providers` | native OpenAI, Azure OpenAI, Anthropic, Google, and fake adapters |
| `agentctl-protocols` | MCP and A2A clients |
| `agentctl-observability` | typed events, test sink, and OpenTelemetry bridge |
| `agentctl-cli` | command parsing, filesystem and process boundary, adapters, output envelopes, exit codes |
| `xtask` | generation, verification, acceptance, container, packaging, and secret checks |

Dependency direction keeps the core free of HTTP, SQLite, CLI, and concrete clock or ID implementations. Concrete effects stay at runtime edges and receive stable effect identity.

## Development setup

Install the pinned Rust 1.88 toolchain with Rustfmt and Clippy, then from the repository root:

```text
cargo build --workspace --locked
cargo test --workspace --all-features --locked
cargo xtask docs-verify
```

Normal development and documentation tests use no provider credential. Install `cargo-deny` 0.20.2 before the complete `cargo xtask verify` gate.

## Deterministic core and effect boundaries

Parsing and compilation must be pure for the same source and inputs. The compiler resolves references, validates capabilities, orders the DAG, and computes a plan digest. The runtime calls injected implementations for clocks, IDs, files, processes, providers, tools, protocols, state, and traces.

An operation that observes or changes non-pure state needs an effect classification and a request record before dispatch. A completed confirmed result can be reused. An unconfirmed started result is uncertain. Do not add automatic retry around ambiguous transport or process failures.

## Runtime state machine

Run and task transitions use enums in `agentctl-core`; the store validates transitions and couples important updates in SQLite transactions. Working-memory replacement, task transition, checkpoint, and audit event commit together. Tool effect and tool-call terminal states also commit together.

Resume continues the same run. Recorded replay creates a linked no-effect record from a terminal source. Fork creates a child run with fresh execution. Any change that alters these semantics needs tests, compatibility notes, and an ADR.

## Providers, tools, MCP, and A2A

Provider adapters implement the neutral model contract and publish typed capabilities. Tools publish strict schemas plus security and recovery metadata. MCP pins `2025-11-25`; A2A pins `1.0`. New protocol behavior needs deterministic local mock peers, bounded timeouts, cancellation, native error mapping, and explicit ambiguous-delivery behavior.

## Testing strategy

- Unit tests cover local invariants and negative contracts.
- Integration tests cover compiler, store, runtime, provider, protocol, policy, and trace boundaries.
- Compatibility fixtures preserve selected language-neutral behavior.
- Property tests cover templates and typed preservation.
- Fuzz targets cover YAML, provider and protocol responses, persisted state, and tool schema input.
- Acceptance tests run clean-directory and packaged user journeys.
- Container acceptance checks the non-root, read-only, state, artifact, signal, and offline replay contract.
- Live OpenAI acceptance is explicit, bounded, credentialed, and never part of normal CI.

## Documentation and release changes

Run `cargo xtask generate` whenever CLI help or the DSL changes. Run `cargo xtask docs-verify` for public content and examples. Update the Pages site's content manifest when adding a new canonical public page. Release decisions depend on hosted evidence for the exact candidate commit; local success alone does not approve an RC.

Related guides: [Add an action](ADD_ACTION.md), [Add a provider](ADD_PROVIDER.md), [Add a store migration](ADD_MIGRATION.md), [Write documentation](DOCUMENTATION.md), [Testing](../TESTING.md), and [Release process](../RELEASE_PROCESS.md).
