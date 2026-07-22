# Observability

The runtime emits versioned typed events for runs, tasks, attempts, agent turns, provider/model responses, tool/effect calls, approvals, MCP/A2A operations, retries, checkpoints, state transitions, and useful database boundaries. Events carry run/task/effect and trace correlation plus phase and timestamp.

`agentctl-observability` provides a no-op sink, buffered test sink, and an OpenTelemetry-compatible global tracer bridge. Tracing is optional and has no role in scheduling or replay. Structured audit events are persisted separately in SQLite and ordered per run.

Sensitive field names and registered secret values are redacted before trace attributes leave the runtime. Provider response content is not printed by the live smoke. Operators must still treat trace backends and the local database as sensitive because prompts, file content, tool output, and remote artifacts may contain confidential non-secret data.

Usage maps input/output/reasoning/cache-read/cache-write tokens where providers expose them. Duration, attempts, provider errors, retries, approval waits, tool counts, and action change status are available from trace and audit events. Price calculation is not fabricated when no reliable price metadata exists.
