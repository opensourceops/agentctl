# Compatibility policy

## Preserved

Declaration-order scheduling among ready tasks, `needs` dataflow, exact typed templates, deterministic assign/assert/file/memory use cases, bounded agent/tool turns, approval concepts, SQLite local persistence, and the useful top-level command names remain. The language-neutral fixture records the legacy assign workflow’s translated model, graph order, and task reference. Omitted `foreach`, `matrix`, and `loop` fields preserve the unchanged single-task graph; compiled expansion metadata is additive.

## Migrated

Unversioned `playbook:` YAML can be translated by `agentctl migrate`; `modules` become `actions`, `module:x` becomes `action:x`, heuristic agents map to the fake provider, and core memory/policy fields are normalized. Rust JSON output is a stable `agentctl.dev/cli/v1` envelope rather than the prototype JSONL/YAML mixture. The production executable and runtime are Rust.

## Intentionally changed

`replay` now means no-effect recorded reconstruction. The prototype operation that created a new effectful run is `fork`. Unknown YAML fields, missing references, cycles, unsupported provider capabilities, invalid tool output, path escapes, unsafe processes/networks, and incompatible durable state now fail explicitly. Direct `--api-key` flags are removed; secret references are required. OpenAI uses current Responses concepts, and Anthropic/Google are native adapters rather than names on an OpenAI-compatible route.

Schema 5 adds selective-repair metadata without changing resume, replay, retry, or fork semantics. New runs persist task fingerprints, output contracts/digests, state deltas, artifacts, and disposition. Older runs migrate and remain inspectable, but tasks completed without metadata version 1 cannot be silently reused by repair.

## Deprecated and removed

Unversioned YAML is compatibility-only and warns. The TypeScript package exposes no `bin` or `main` and is archived. Placeholder memory adapters, provider environment-name-only “support,” YAML output, legacy profiles, automatic endpoint overrides, old prompt-cache fields, and optimistic replay semantics are removed from production.

Tool-level `compensation` metadata was never executable and is rejected. Declare
an effectful inverse action on each source task with `compensate`; see
[Compensation](guides/COMPENSATION.md).

Free-form `team:` orchestration is rejected. Convert each role to an explicit
agent task and each payload transfer to a typed handoff task; see
[Structured role handoffs](guides/STRUCTURED_HANDOFFS.md).

Legacy workflows depending on packs, broad built-in tool profiles, remote MCP/A2A shape, MongoDB memory, provider-specific endpoint fields, or embedded credentials require manual conversion. The translator intentionally refuses to guess security-sensitive intent.

## Separate product decisions

A public pack registry/resolver, vector memory, automatic MCP reconnection, general A2A resubmission, and streamed model output are not compatibility promises for v1alpha1. Bounded loops, namespaced sub-workflows, explicit source-linked compensation, and graph-native structured handoffs are additive; hidden or model-controlled orchestration is intentionally unsupported.
