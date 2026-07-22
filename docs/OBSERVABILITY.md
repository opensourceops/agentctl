# Observability

Observability has two audiences: terminal users need a concise final result, while operators need durable evidence for diagnosis and audit. The runtime keeps those contracts separate.

## CLI streams

Human output is the default for interactive use. For automation, use one versioned JSON document:

```bash
agentctl run workflow.yaml --db .agentctl/runtime.db --output json --color never
```

Success writes an `agentctl.dev/cli/v1` envelope to stdout. Failure writes the same envelope shape to stderr and returns a typed exit code. Run-scoped results include run and trace IDs. JSONL progress streaming is not implemented in this release; do not parse human output or assume that each line is an event.

## Durable inspection

The SQLite database is the authoritative local record. Inspect a run and database without invoking a provider or repeating an effect:

```bash
agentctl inspect RUN_ID --db .agentctl/runtime.db --output json --color never
agentctl db stats --db .agentctl/runtime.db --output json --color never
```

Inspection includes task attempts, checkpoints, effect state, approvals, provider and protocol records, ordered audit events, and trace correlation. Use `agentctl approvals list RUN_ID` when the run exited pending approval. Preserve the database and its WAL files together when the history is operational evidence.

## Runtime events

The runtime emits versioned typed events for runs, tasks, attempts, agent turns, provider/model responses, tool/effect calls, approvals, MCP/A2A operations, retries, checkpoints, state transitions, and useful database boundaries. Events carry run, task, effect, and trace correlation plus phase and timestamp.

`agentctl-observability` provides a no-op sink, buffered test sink, and an OpenTelemetry-compatible global tracer bridge. Tracing is optional and has no role in scheduling or replay. Structured audit events are persisted separately in SQLite and ordered per run.

OpenTelemetry export is an embedding concern in this release; the standalone CLI does not expose an exporter configuration flag. An application using the runtime can install the bridge and route spans through its own collector configuration. A tracing outage must not alter workflow scheduling or replay semantics.

## Metrics and interpretation

Usage maps input, output, reasoning, cache-read, and cache-write tokens where providers expose them. Duration, attempts, provider errors, retries, approval waits, tool counts, and action change status are available from trace and audit events. Price calculation is not fabricated when no reliable price metadata exists.

When diagnosing a failure, correlate the final envelope's run and trace IDs with the persisted task, attempt, effect, and provider records. A model response is not proof that an external effect completed; use the effect record and its confirmation state.

## Sensitive data

Sensitive field names and registered secret values are redacted before trace attributes leave the runtime. Provider response content is not printed by the live smoke. Operators must still treat trace backends and the local database as sensitive because prompts, file content, tool output, and remote artifacts may contain confidential non-secret data.

Keep provider credentials in environment references, never workflow inputs or command arguments. Apply access control and retention to the database, collected artifacts, CI logs, and trace backend. Before sharing diagnostics, remove credentials, prompt content, file content, remote payloads, and identifying metadata; a run ID alone is sufficient for local correlation.

See [CLI output and exit codes](reference/CLI_OUTPUT.md), [local operation](guides/LOCAL_OPERATION.md), and [runtime database and migrations](reference/DATABASE.md) for the complete operating contract.
