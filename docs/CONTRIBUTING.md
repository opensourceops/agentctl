# Contributing

Thank you for improving `agentctl`. Keep changes narrow, add executable evidence for public behavior, and preserve the deterministic and security boundaries.

## Before you start

Read the [code of conduct](../CODE_OF_CONDUCT.md), [product definition](PRODUCT.md), [architecture](ARCHITECTURE.md), [security model](SECURITY.md), and [limitations](LIMITATIONS.md). Search existing issues before proposing new work. Use an issue to discuss large compatibility, protocol, persistence, or security changes before implementation.

Do not use a public issue for a vulnerability. Follow the private process in [SECURITY.md](../SECURITY.md).

## Choose work

Prefer a scoped issue with expected behavior. For a bug, add a failing test that reproduces the user-visible problem before the fix. For a feature, define validation, policy, persistence, recovery, compatibility, and documentation effects.

The maintainers do not promise response or review times.

## Set up development

Install the pinned Rust 1.88 toolchain with Rustfmt and Clippy. From the repository root:

```text
cargo build --workspace --locked
cargo test --workspace --all-features --locked
cargo xtask docs-verify
```

Normal tests need no provider credential. Install the pinned `cargo-deny` version documented in `.github/workflows/ci.yml` before running the complete verification gate.

## Branches and commits

Create a focused feature branch. Keep unrelated formatting, renames, and refactors out of the change. Write commits that explain one coherent behavior or documentation change. Generated schema and CLI reference changes belong with the source change that caused them.

Do not commit runtime databases, build output, provider responses, API keys, local absolute paths, or private release evidence.

## Build and test

Before requesting review:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo xtask generate
cargo xtask docs-verify
cargo xtask verify
cargo xtask acceptance
git diff --check
```

Run `cargo xtask acceptance-container` when a container, Containerfile, mount contract, signal path, filesystem behavior, or packaging boundary changes. Live OpenAI acceptance is an explicit credentialed release gate, not a normal contribution requirement.

## Special changes

- Actions and tools: follow [Add an action or tool](development/ADD_ACTION.md).
- Providers: follow [Add a provider](development/ADD_PROVIDER.md).
- MCP or A2A: add pinned-protocol mock coverage, timeouts, cancellation, policy, redaction, and ambiguous-delivery behavior.
- Store migrations: follow [Add a store migration](development/ADD_MIGRATION.md).
- Workflow or durable compatibility: update fixtures, generated schema, public policy, and an ADR when the decision is architectural.
- Documentation and examples: follow [Write and verify documentation](development/DOCUMENTATION.md).

## Pull request checklist

- The change is linked to a clear problem.
- Tests fail before the fix where practical and pass after it.
- Security, policy, effect, recovery, and redaction behavior is explicit.
- No live credential is needed by normal CI.
- Generated files are current.
- Public examples are executable and use fake providers or local mocks by default.
- Compatibility and limitations are updated.
- New public writing contains no em dash and uses precise maturity language.
- The diff contains no unrelated cleanup.

## Review expectations

Reviewers focus on correctness, deterministic behavior, explicit effects, safe failure, compatibility, tests, and truthful documentation. Address each review comment with a change or a concrete technical explanation. A local pass does not replace hosted evidence for an RC.

## Release process

Maintainers follow [Release process](RELEASE_PROCESS.md). Candidate promotion requires the exact remote commit to pass required hosted checks and artifact verification. Contributors must not create tags, publish packages, or describe a local build as released.
