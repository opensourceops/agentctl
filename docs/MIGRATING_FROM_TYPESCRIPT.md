# Migrating from the TypeScript prototype

1. Preserve a copy of the old workflow and run the archived test suite if its behavior matters: `NODE_OPTIONS=--no-deprecation npm test`.
2. Run `cargo run -p agentctl -- migrate old.yaml --write workflow.yaml`.
3. Run `agentctl check workflow.yaml` and address every diagnostic; the new schema is strict.
4. Replace `module:name` with `action:name`, and define typed provider entries referenced by agents.
5. Move credentials to `{ env: NAME }`; remove API-key arguments and inline tokens. Add environment, provider, host, process, readable workspace, and writable-root policy grants explicitly.
6. Replace the old effectful meaning of replay with `fork`. Use `replay` only when no current external observation is desired.
7. Review model settings: OpenAI is Responses-native, `reasoning.effort` uses current values, tools require strict input/output schemas, and token/tool/turn/time bounds are mandatory/defaulted.
8. Convert MCP to `mcpServers` version `2025-11-25` and A2A to `a2aPeers` version `1.0`; secrets belong in header references.
9. Verify with the fake provider and local mocks. Run any `*-live.yaml` example only as an explicit external test.

The automatic translator covers simple top-level metadata, modules/actions, tasks, heuristic agents, common approval mode, and initial working memory. It discards unsupported legacy provider endpoint/cache/profile fields with a migration warning rather than preserving unsafe or obsolete semantics. Pack-backed actions, remote transports, MongoDB/vector memory, arbitrary profiles, and custom TypeScript executors must be rewritten against the Rust contracts.

Use `fixtures/compat/v0/assign.playbook.yaml` as the minimum preserved contract and compare changes against [Compatibility](COMPATIBILITY.md). The old source is non-production reference material; do not add new behavior to it.
