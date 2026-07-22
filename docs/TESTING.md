# Testing and verification

The canonical command is:

```console
cargo xtask verify
```

User-journey layers are separate:

```console
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask acceptance-live-openai  # explicit credentialed gate only
cargo xtask package
```

It checks rustfmt; clippy with all targets/features and warnings denied; locked build; unit, integration, compatibility, provider, protocol, persistence, runtime, and security tests; rustdoc; generated schema/CLI consistency; all workflow validation and deterministic examples; negative capability/policy/no-mutation cases; dependency sources/licenses/advisories; a repository secret-pattern scan; `cargo install`; and the Rust-only production boundary.

Unit tests cover parser diagnostics, strictness, compiler order/cycles/capabilities, templates, tool schemas, policy traversal/network/redaction, state transitions, effect recovery, store migration/corruption/checkpoints, runtime dataflow/check/diff/approval/cancellation/replay/fork, provider mappings, protocols, and traces. `proptest` exercises arbitrary templates and typed preservation. Language-neutral fixtures in `fixtures/compat` preserve the TypeScript oracle’s external graph/dataflow contract.

`fuzz/` contains `cargo-fuzz` targets for workflow YAML/templates, provider responses, MCP/A2A payload shapes, persisted state, and tool schemas/inputs. They use no network or credentials. Example:

```console
cargo install cargo-fuzz
cargo fuzz run workflow_yaml -- -max_total_time=60
```

CI runs the canonical suite on Linux, macOS, and Windows, stable and Rust 1.88, plus credential-free acceptance, a Linux amd64 container gate, and strict supply-chain checks. Provider/protocol conformance uses local mock HTTP servers. Normal examples are deterministic; MCP/A2A runtime behavior is covered by mocks rather than requiring a background service.

The only full live gate is the separately invoked OpenAI acceptance described in [Providers](PROVIDERS.md). It performs two bounded Responses API requests locally and two in the OCI image for one tool-call/continuation journey each, then performs keyless replays. Never run it for debugging loops, fuzzing, load, or normal CI.
