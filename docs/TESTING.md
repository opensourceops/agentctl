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
cargo xtask resource-budget-live-openai  # one-request credentialed gate only
cargo xtask examples-verify
cargo xtask examples-verify-live-openai  # explicit credentialed gate only
cargo xtask package
cargo xtask secret-scan
```

It checks rustfmt; clippy with all targets/features and warnings denied; locked build; unit, integration, compatibility, provider, protocol, persistence, runtime, and security tests; rustdoc; generated schema/CLI consistency; all workflow validation and deterministic examples; negative capability/policy/no-mutation cases; dependency sources/licenses/advisories; repository secret patterns and immutable workflow action pins; `cargo install`; and the Rust-only production boundary.

Unit tests cover parser diagnostics, strictness, compiler order/cycles/capabilities, templates, tool schemas, policy traversal/network/redaction, state transitions, effect recovery, store migration/corruption/checkpoints, runtime dataflow/check/diff/approval/cancellation/replay/repair/fork, provider mappings, protocols, and traces. Repair regressions cover two-agent reuse with a panic-on-repeat provider, downstream and branch closure, repeated roots, changed definitions/prompts, output/state/artifact corruption, migration and rollback, effect uncertainty/reconciliation, approval gating, source garbage collection, and effect-free replay. `proptest` exercises arbitrary templates and typed preservation. Language-neutral fixtures in `fixtures/compat` preserve the TypeScript oracle’s external graph/dataflow contract.

`fuzz/` contains `cargo-fuzz` targets for workflow YAML/templates, provider responses, MCP/A2A payload shapes, persisted state, and tool schemas/inputs. They use no network or credentials. Example:

```console
cargo install cargo-fuzz
cargo fuzz run workflow_yaml -- -max_total_time=60
```

Hosted CI runs the canonical suite, credential-free acceptance, and packaging
on Rust 1.88 for Linux x64, macOS arm64, and Windows x64. Separate automatic
jobs cover the Linux x64 container, current vulnerability scan, two CycloneDX
SBOM artifacts, complete-history/tree secret scans, dependency policy, and
workflow lint. Exact-head pull-request and release-preparation runs provide
validated hosted-platform evidence. Provider/protocol conformance uses local
mock HTTP servers. Normal examples are deterministic; MCP/A2A runtime behavior
is covered by mocks rather than requiring a background service.

Live gates are separately invoked and described in [Providers](PROVIDERS.md).
The original acceptance performs one tool-call/continuation journey locally
and in the image. `resource-budget-live-openai` performs exactly one provider
dispatch, then proves that the next requested effect is denied.
`examples-verify-live-openai` inventories and runs every OpenAI-backed example,
including the failed two-agent source, selective repair, and keyless replay,
with a 40-request and conservative USD 10 guard. Never run these commands for
debugging loops, fuzzing, load, or normal CI.
