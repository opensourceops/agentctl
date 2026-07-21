# Prompt Cache Example

This example uses a local mock OpenAI-compatible Responses endpoint to show provider-native prompt-cache behavior.

It proves:

- playbook-level prompt cache config
- grouped multi-agent cache sharing
- deterministic task verification
- runtime cache metrics visible through `agentctl prompt-cache stats`

## Run it

Start the mock server in one terminal:

```bash
node examples/prompt-cache/mock-openai-server.mjs
```

Then run the playbook in another:

```bash
agentctl run examples/prompt-cache/mission.playbook.yaml --db .runtime/prompt-cache-example.db --api-key test-key
```

Inspect the cache metrics:

```bash
agentctl prompt-cache stats --db .runtime/prompt-cache-example.db --verbose
```

Expected behavior:

- the run succeeds
- the mock server prints the same `prompt_cache_key` for both agent requests
- the second response reports cached tokens
- `prompt-cache stats` shows:
  - `totalResponses: 2`
  - `hitResponses: 1`
