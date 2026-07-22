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

## Manual migration/deferred

Legacy pack-backed workflows, custom TypeScript executors, MongoDB/vector memory, and old MCP/A2A shapes require manual conversion. Parallel/dynamic workflows, sub-workflows, teams/handoffs, compensation execution, registry resolution, and automatic remote resubmission are deferred product decisions.
