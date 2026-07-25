# ADR 0014: Structured handoffs as graph data

- Status: accepted
- Date: 2026-07-25

## Context

Named agent roles are useful, but a hidden team conversation would introduce a
second scheduler, undeclared routing, shared mutable transcript state, and
recovery semantics outside the compiled workflow.

## Decision

The supported composition model is explicit role tasks and typed handoff tasks.
Each role is an ordinary bounded agent task. Each handoff is a deterministic
task with an output schema, explicit sender and recipient, and normal graph
dependencies. Reusable role graphs use existing typed sub-workflows.

Agent definitions own provider, model, instructions, visible tools, turn and
tool-call bounds, usage bounds, timeout, retry, and structured output. Workflow
policy and tool approvals remain non-bypassable.

The compiler rejects `uses: team:<name>` with guidance to use agent tasks,
handoff tasks, routers, and sub-workflows.

## Consequences

- Handoffs use ordinary task persistence, output digests, checkpoints, audit,
  and traces.
- Retry and repair can reuse compatible upstream roles and handoffs.
- Recorded replay creates no provider or tool effect.
- Typed routers express conditional recipients without model-owned routing.
- No new database schema, nested runtime, mailbox, or shared conversation state
  is required.
- Free-form multi-agent conversation remains outside the product surface.
