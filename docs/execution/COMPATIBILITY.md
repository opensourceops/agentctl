# Compatibility ledger

The TypeScript oracle is commit `be9d0ae`; the detailed public policy is [docs/COMPATIBILITY.md](../COMPATIBILITY.md).

## Preserved

- Declaration-order DAG scheduling, dependencies, typed exact templates, deterministic dataflow, bounded agents, approvals, and SQLite local state.
- Simple legacy assign behavior is captured in `fixtures/compat/v0` and dual-mode language-neutral assertions.

## Migrated

- `playbook` to versioned `apiVersion`/`kind`/`metadata`/`spec`; `modules` to `actions`; `module:x` to `action:x`.
- Rust versioned JSON output, generated JSON Schema/CLI reference, native providers/protocols, and namespaced dotted packs.

## Intentionally changed

- Recorded `replay` cannot call effects; legacy effectful replay is now `fork`.
- Strict unknown fields, capability/schema validation, formal run/task states, request-before-start effects, migration failures, safe environment references, and denied redirects replace permissive prototype behavior.
- Anthropic and Google use their native APIs; current OpenAI uses Responses concepts.

## Deprecated

- Unversioned YAML is warning-only compatibility input for simple workflows.
- TypeScript source/tests are archived and expose no production entry point.

## Removed

- Direct API-key flags, YAML machine output, legacy profiles, placeholder provider/memory support, optimistic replay, implicit full environment inheritance, and obsolete provider/cache fields.
- Non-executable tool-level `compensation` metadata. Declare the inverse action
  on an effectful task with `compensate`.

## Manual migration

Legacy custom TypeScript executors, MongoDB memory, and old MCP/A2A shapes
require manual conversion. Bounded parallel and dynamic tasks, sub-workflows,
and source-linked compensation are additive. A public registry is an explicit
non-goal. Free-form teams migrate to explicit bounded role tasks and typed
handoff tasks. MCP actions may add an explicit idempotency declaration for
bounded schema-checked reconnect. A2A peers may add polling bounds. Existing
documents retain conservative defaults, and ambiguous remote submission is
never repeated.

Bounded provider streaming is additive. Existing agents default to
`stream: false`, final JSON remains one document, and JSONL uses the existing
versioned envelope. Stream transport loss never weakens at-most-once recovery.
