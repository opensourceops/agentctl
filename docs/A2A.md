# A2A support

The client pins [A2A `1.0`](https://a2a-protocol.org/latest/specification/) and discovers an Agent Card. It selects a same-origin JSON-RPC interface advertising version `1.0`, then supports `SendMessage`, bounded `GetTask` polling, `SubscribeToTask` SSE updates, `CancelTask`, terminal states, messages, structured parts, and artifacts.

Card and RPC authentication headers are secret references resolved at dispatch and refreshed once after `401`. The card URL is subject to network policy, redirects are disabled, and the selected interface must have the same scheme, host, and effective port as that reviewed card URL. Cards, skills, messages, parts, and artifacts are untrusted data. An A2A delegation is an `at_most_once` `remote_agent` effect and may require approval.

Configure observation bounds explicitly when the defaults are unsuitable:

```yaml
a2aPeers:
  worker:
    cardUrl: https://agents.example.test/card.json
    protocolVersion: "1.0"
    timeoutSeconds: 10
    maxPolls: 100
    pollIntervalMs: 100
```

The runtime persists the call identity and remote task ID before polling. A lost polling or streaming connection can refresh the same-origin Agent Card once and resume observation of that task. It never sends another `SendMessage`. If the submission response itself was ambiguous and no task ID was received, automatic continuation is refused.

Continue a known uncertain task with:

```text
agentctl effects --db .agentctl/runtime.db continue-remote EFFECT_ID \
  --actor operator --reason "resume persisted task" --approved
```

The command observes the existing remote task, ingests completed inline or same-origin URL artifacts into the local CAS, and records an applied effect reconciliation. A following failed-only retry materializes that completed boundary and executes only its descendants. Repair, retry, and replay retain source-linked protocol evidence without submitting the task again.

Each artifact part must contain exactly one of `text`, `raw`, `data`, or `url`. Retrieval is bounded to 16 MiB per part and same-origin URL policy. Mock peers cover known-task continuation, ambiguous-send refusal, task polling, artifacts, streaming fallback, cancellation, protocol mismatch, origin enforcement, timeout, and zero-resubmission retry.
