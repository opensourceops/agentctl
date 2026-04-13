# TypeScript Conventions

`agentctl` uses strict TypeScript as part of the runtime contract, not just as editor help.

## Core Rules

- Prefer `unknown` to `any`.
- Prefer explicit `JsonObject` and `JsonValue` types for runtime payloads.
- Keep mutable state narrow and local.
- Make public interfaces explicit.
- Use type guards instead of broad assertions where runtime data crosses process or network boundaries.

## Runtime Boundaries

These parts of the codebase are treated as untrusted-input boundaries and should always normalize or validate data before use:

- CLI argument parsing
- YAML playbook parsing
- provider responses
- MCP and A2A transport responses
- long-term memory adapters

Do not pass raw `JSON.parse(...)` results or remote payloads deeper into the runtime without narrowing them first.

## JSON Payload Model

The canonical JSON types live in [types.ts](/Users/ompragash/Git/agentctl/src/types.ts):

- `JsonPrimitive`
- `JsonArray`
- `JsonObject`
- `JsonValue`

Use:

- `JsonObject` for structured runtime inputs, task inputs, memory updates, and provider state
- `JsonValue` for generic serialized payloads

Avoid introducing ad hoc `Record<string, unknown>` payloads when the data is already part of the runtime JSON model.

## Memory and Task State

- `memory.working` is the canonical mutable per-run memory surface.
- `vars` remains a compatibility mirror only.
- long-term memory adapters should return plain JSON-shaped entries that can be promoted into working memory without unsafe casting.

## Module Design

Builtin modules should:

- validate inputs up front
- return structured `TaskOutput`
- return `stateUpdates` only when they intentionally mutate working memory
- avoid side-effecting shared state outside the returned `stateUpdates`

## Agent Design

Agent models should:

- keep provider-specific state isolated in `providerState`
- treat tool arguments as validated `JsonObject`
- avoid relying on implicit truthy/falsy casting for control flow when the shape is known

## Refactor Notes

Recent refactors in this repo tightened:

- JSON object typing in module and agent execution paths
- long-term memory retrieval promotion behavior
- MongoDB Atlas adapter aggregation typing

When extending these areas, preserve the same pattern:

1. narrow the input
2. keep the runtime payload JSON-shaped
3. add a regression test for the exact boundary you changed
