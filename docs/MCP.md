# MCP support

The client pins the stable MCP protocol version `2025-11-25` from the [official specification](https://modelcontextprotocol.io/specification/2025-11-25). It uses Streamable HTTP JSON-RPC and implements initialization/version negotiation, initialized notification, session IDs, protocol headers, tool listing, input/output schemas, tool calls, structured content, bounded SSE response parsing, timeout, and best-effort cancellation notification.

Authentication headers are secret references resolved at dispatch. A `401` refreshes those references once for initialization, notification, listing, or call. The endpoint must pass network policy. Redirects are disabled and an `Origin` header is sent. Remote descriptions, schemas, content, and annotations are untrusted; annotations are exposed only as metadata and never authorize an effect.

Set `idempotency` on an `mcp.call` action when its server contract justifies it:

```yaml
actions:
  lookup:
    kind: mcp.call
    idempotency: idempotent
```

`pure`, `idempotent`, and `keyed` calls may reconnect once after session expiry, timeout, transport loss, or a malformed response. A keyed call also sends the stable effect identity as `params._meta["agentctl.dev/idempotency-key"]`; the declaration is valid only when the reviewed server contract honors that key. Reconnection creates a new session, refreshes `tools/list`, and compares the selected tool's schema digest before redispatch. A changed or missing schema fails without a second call. `at_most_once` and `unknown` calls are never redispatched after an ambiguous response.

SQLite schema 13 records session generation, immutable call identity, idempotency, status, selected remote, and encrypted-capable protocol state. `agentctl inspect RUN_ID` exposes `protocolSessions`, `protocolCalls`, and bounded protocol stream events. Recorded replay copies source-linked protocol evidence but performs no network effect.

Streaming progress is persisted with backpressure before the next frame is consumed. A final tool result enters workflow state only after complete JSON-RPC validation. Deterministic mock-server coverage includes server restart, one-reconnect bounds, stable and changed schemas, authentication refresh, unsafe-call refusal, SSE parsing, cancellation, timeout, inspection, and replay.
