# Contributing

Use the pinned Rust toolchain and keep changes scoped to the deterministic product. Before editing a public contract, add or update a fixture/test and an ADR when durability, security, compatibility, dependency direction, or protocol version changes.

```console
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo xtask generate
cargo xtask verify
```

Generated schema and CLI reference must be committed. No test, example, benchmark, fuzz target, or CI job may require provider credentials. Do not add raw keys, secret CLI flags, redirects, shell-string execution, unbounded retries/turns, or implicit effects. New providers require native mapping, capabilities, normalized errors/usage/cancellation, documentation, example configuration, and mock conformance. New tools require both schemas, risk/effect/idempotency/approval metadata, policy hooks, and malicious-output tests.

Dependencies must be registry releases with reviewed licenses and no wildcard constraints. Unsafe Rust is forbidden. Cross-platform behavior belongs in the CI matrix. Update `docs/execution` with exact evidence when finishing a release gate.
