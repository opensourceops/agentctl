# Compatibility policy

## Preserved

Declaration-order scheduling among ready tasks, `needs` dataflow, exact typed templates, deterministic assign/assert/file/memory use cases, bounded agent/tool turns, approval concepts, SQLite local persistence, and the useful top-level command names remain. The language-neutral fixture records the legacy assign workflow’s translated model, graph order, and task reference. Omitted `foreach` and `matrix` fields preserve the unchanged single-task graph; compiled expansion metadata is additive.

## Migrated

Unversioned `playbook:` YAML can be translated by `agentctl migrate`; `modules` become `actions`, `module:x` becomes `action:x`, heuristic agents map to the fake provider, and core memory/policy fields are normalized. Rust JSON output is a stable `agentctl.dev/cli/v1` envelope rather than the prototype JSONL/YAML mixture. The production executable and runtime are Rust.

## Intentionally changed

`replay` now means no-effect recorded reconstruction. The prototype operation that created a new effectful run is `fork`. Unknown YAML fields, missing references, cycles, unsupported provider capabilities, invalid tool output, path escapes, unsafe processes/networks, and incompatible durable state now fail explicitly. Direct `--api-key` flags are removed; secret references are required. OpenAI uses current Responses concepts, and Anthropic/Google are native adapters rather than names on an OpenAI-compatible route.

Schema 5 adds selective-repair metadata without changing resume, replay, retry, or fork semantics. New runs persist task fingerprints, output contracts/digests, state deltas, artifacts, and disposition. Older runs migrate and remain inspectable, but tasks completed without metadata version 1 cannot be silently reused by repair.

## Deprecated and removed

Unversioned YAML is compatibility-only and warns. The TypeScript package exposes no `bin` or `main` and is archived. Placeholder memory adapters, provider environment-name-only “support,” YAML output, legacy profiles, automatic endpoint overrides, old prompt-cache fields, and optimistic replay semantics are removed from production.

Legacy workflows depending on packs, broad built-in tool profiles, remote MCP/A2A shape, MongoDB memory, provider-specific endpoint fields, or embedded credentials require manual conversion. The translator intentionally refuses to guess security-sensitive intent.

## Deferred product decisions

Loops, sub-workflows, teams/handoffs, compensation execution, a public pack registry/resolver, vector memory, automatic MCP reconnection, general A2A resubmission, and streamed model output are not compatibility promises for v1alpha1.
