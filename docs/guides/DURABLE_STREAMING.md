# Use durable provider streaming

Set `stream: true` on an agent to request provider progress events:

```yaml
agents:
  reporter:
    provider: openai
    model: gpt-5.6
    instructions: Return the report.
    stream: true
```

The fake, OpenAI, and Azure OpenAI providers advertise the streaming
capability. Anthropic and Google agents with `stream: true` fail compilation
until their native streaming transports implement this contract.

The canonical credential-free example is
[`streaming.yaml`](../../examples/v1/streaming.yaml).

## Durable event contract

Each accepted provider event is persisted before the adapter consumes more
network data. This synchronous boundary applies backpressure to the producer.
Records include run ID, task ID, task attempt, a monotonic task-attempt
sequence, optional provider sequence, effect ID, event type, bounded payload,
truncation state, and timestamp.

One task attempt retains at most 256 events. One stored payload retains at most
4 KiB of JSON. An oversized payload is replaced by its byte length and SHA-256
digest. The OpenAI adapter accepts at most 8 MiB for the complete SSE
transport. Sensitive field names and configured provider credential values are
redacted before persistence. Stream payloads participate in selected-field
state encryption when encryption is enabled.

These bounds apply to progress records, not final result validation. The
provider must still return a complete terminal response. Structured output,
usage limits, finish reasons, tool calls, and task output schemas are validated
through the same path as a non-streaming call.

## CLI output modes

- `--output human` writes stream progress to stderr and the final summary to
  stdout.
- `--output jsonl` writes one versioned `StreamEvent` envelope per durable
  event, followed by one final outcome envelope.
- `--output json` writes exactly one final JSON document and never mixes
  progress into stdout or stderr.

All three modes persist the same stream records. Use `agentctl inspect RUN_ID`
to read them under `streamEvents`.

## Cancellation and transport loss

Cancellation stops SSE consumption and preserves accepted events. A timeout,
transport loss, malformed terminal event, or persistence failure after
dispatch makes the at-most-once model effect uncertain. `agentctl` does not
reconnect or resubmit that request automatically. Inspect and reconcile the
effect before deciding whether fresh execution is safe.

This is intentionally different from reconnecting a read-only feed. A model
request may have completed remotely even when the local process did not
receive its terminal response.

## Recorded replay

Recorded replay copies the bounded stream records into the replay run with
source run and source sequence linkage. JSONL and human replay can render those
copied events. Replay dispatches no provider or tool effect.

Partial model output can contain confidential content and can be harder to
moderate than a complete response. Redaction is not a content-classification
system. Apply database access control and retention appropriate for prompts
and model output. See the official [OpenAI streaming
guide](https://developers.openai.com/api/docs/guides/streaming-responses?api-mode=responses).
