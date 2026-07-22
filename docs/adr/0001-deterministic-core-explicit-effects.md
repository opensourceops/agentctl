# ADR 0001: Deterministic core and explicit effects

Status: accepted, 2026-07-22.

The graph, policy, state machine, persistence decisions, and recorded replay remain model-independent. Provider, tool, filesystem, process, MCP, A2A, internal-state mutation, clock, and ID behavior cross injected interfaces and receive durable effect identity when externally observable.

This keeps models replaceable and tests credential-free. It requires more records and conservative uncertain states, but avoids hidden calls during replay. Dynamic model-owned orchestration is rejected.
