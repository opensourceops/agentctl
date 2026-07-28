# ADR 0015: Durable stream events

- Status: accepted
- Date: 2026-07-25

## Context

Provider streaming reduces time to first progress, but forwarding raw deltas
directly to a terminal would bypass durable history, permit unbounded output,
mix progress with final JSON, and leave cancellation or replay behavior
undefined.

## Decision

Streaming is an explicit agent capability. Providers emit typed fragments
through an awaited sink. The runtime redacts and bounds each fragment, persists
it under a task-attempt sequence, and only then permits the provider adapter to
continue consuming the transport.

Human and JSONL modes may render persisted progress. Final JSON mode emits only
one final document. Terminal provider responses still pass the ordinary tool,
finish-reason, usage, structured-output, and task-output validation path.

Recorded replay copies stream records with source linkage and dispatches no
effect. A dropped or malformed stream after dispatch is uncertain and is never
automatically reconnected or resubmitted.

## Consequences

- SQLite schema 12 adds encrypted-capable bounded stream records.
- One task attempt retains at most 256 events and 4 KiB per event payload.
- OpenAI Responses SSE is supported for OpenAI and Azure OpenAI; the fake
  provider supplies deterministic verification.
- Anthropic and Google streaming fail capability negotiation.
- New provider event types remain data rather than changing workflow control
  flow.
- Progress records are evidence, not a substitute for a valid terminal result.
