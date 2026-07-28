# ADR 0021: Durable run budget ledger

## Status

Accepted.

## Context

Per-agent and per-process bounds do not cap a composed run. Parallel tasks also
need one authority for scarce request, token, tool, process-output, artifact,
wall-time, and monetary units. In-memory counters lose state on crash and can
oversubscribe under parallel dispatch.

## Decision

The compiled plan carries optional run limits, versioned custom pricing, and
static task, expansion, and loop counts. SQLite stores one usage and
reservation ledger per run. Fresh effects reserve known upper bounds in an
immediate transaction before dispatch and reconcile actual usage afterward.
Exact equality is permitted. A reservation that would exceed a limit is
denied and audited.

Provider input size and future output or token classes are estimates before a
response. Monetary enforcement requires authoritative provider cost or
explicit integer custom rates. Token-only limits remain available without
pricing. The wall deadline is derived from the durable run creation time.

Replay and reused repair or retry boundaries do not consume fresh effect
units. Every derived run owns a separate ledger.

## Consequences

Parallel dispatch cannot oversubscribe a configured unit. Crash recovery can
identify active or reconciled reservations by stable effect identity.
Conservative reservations can reject work whose configured maximum is larger
than the remaining run allowance even if its eventual usage might have been
smaller. Actual usage can exceed an estimate only after the external response;
the response remains durable and the run then fails before another dispatch.
