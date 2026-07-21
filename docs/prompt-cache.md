# Prompt Cache

`agentctl` treats prompt cache as a provider-native optimization layer.

It is not:

- run memory
- working memory
- long-term memory
- a correctness dependency

It is currently implemented for:

- `agents.<name>.kind: openai.responses`
- `provider: openai`

It is not currently supported for:

- `builtin.heuristic`
- `provider: azure-openai-responses`

## What prompt cache does

When enabled, the OpenAI adapter sends a stable `prompt_cache_key` with each response request and records cache-usage metrics from the provider response.

`agentctl` then exposes those metrics through:

- runtime audit events
- `agentctl prompt-cache stats`

Prompt cache is runtime-owned. Agents do not read or write cache contents directly.

## Default behavior

Prompt cache is disabled by default.

That is deliberate:

- it is optimization, not correctness
- provider semantics differ
- hidden cache reuse can make debugging harder
- some deployments will treat cached prompt material as sensitive

## Configuration

Prompt cache can be configured at:

- playbook level: `promptCache`
- agent level: `agents.<name>.promptCache`

Agent-level config overrides playbook-level defaults.

Example:

```yaml
promptCache:
  enabled: true
  retention: in_memory
  keyScope: agent

agents:
  reviewer:
    kind: openai.responses
    provider: openai
    model: gpt-5-mini
    instructionsFile: ./prompts/review.md
```

## Supported fields

```yaml
promptCache:
  enabled: true | false
  force: true | false
  retention: in_memory | 24h
  keyScope: agent | run | playbook | provider | custom
  shareMode: isolated | group
  group: optional string
  keyTemplate: optional string
```

Rules:

- `shareMode: group` requires `group`
- `keyScope: custom` requires `keyTemplate`
- custom OpenAI-compatible `baseUrl` values disable prompt cache unless `force: true`

## Key generation

`agentctl` generates a stable cache key from:

- provider identity
- configured sharing subject
- a static fingerprint of the agent prompt prefix

That prefix fingerprint includes:

- agent kind
- provider
- model
- instruction template content
- tool names and tool input-key shape

This keeps grouped sharing safer by only aligning agents that actually have the same stable prompt prefix.

## Scope and sharing

The effective sharing subject works like this:

### `shareMode: isolated`

`keyScope` determines the sharing boundary:

- `agent`
  - isolated per agent
  - best default
- `run`
  - shared within one run
- `playbook`
  - shared across runs of the same playbook
- `provider`
  - shared broadly for the provider path
- `custom`
  - uses `keyTemplate`

### `shareMode: group`

When `shareMode: group` is used, the `group` name becomes the sharing subject.

Use this only when multiple agents intentionally share the same prompt prefix.

Example:

```yaml
agents:
  planner:
    kind: openai.responses
    provider: openai
    model: gpt-5-mini
    instructions: shared review prefix
    promptCache:
      enabled: true
      shareMode: group
      group: review-shared

  reviewer:
    kind: openai.responses
    provider: openai
    model: gpt-5-mini
    instructions: shared review prefix
    promptCache:
      enabled: true
      shareMode: group
      group: review-shared
```

## Retention

Supported values:

- `in_memory`
- `24h`

For OpenAI-native caching:

- `in_memory` is the baseline retention
- `24h` is only requested where the provider path supports it

On non-direct OpenAI base URLs:

- prompt cache is disabled by default
- `force: true` is required to opt in
- when forced, `24h` retention is downgraded to in-memory retention

## Single-agent example

```yaml
playbook: prompt-cache-single

promptCache:
  enabled: true
  force: true
  retention: in_memory
  keyScope: agent

agents:
  review:
    kind: openai.responses
    provider: openai
    model: gpt-5-mini
    instructionsFile: ./prompts/review.md

tasks:
  - id: run_review
    uses: agent:review
```

## Multi-agent grouped example

```yaml
playbook: prompt-cache-group

agents:
  first:
    kind: openai.responses
    provider: openai
    model: gpt-5-mini
    instructions: shared cache prefix
    promptCache:
      enabled: true
      shareMode: group
      group: shared-reviewers

  second:
    kind: openai.responses
    provider: openai
    model: gpt-5-mini
    instructions: shared cache prefix
    promptCache:
      enabled: true
      shareMode: group
      group: shared-reviewers

tasks:
  - id: one
    uses: agent:first

  - id: two
    needs: [one]
    uses: agent:second
```

## Observability

Inspect recorded cache metrics with:

```bash
agentctl prompt-cache stats
agentctl prompt-cache stats --db .runtime/runtime.db --output json
agentctl prompt-cache stats --run-id <run-id> --verbose
agentctl prompt-cache stats --task-id review
```

The stats output includes:

- total responses
- hit responses
- cached input tokens
- uncached input tokens
- total input tokens
- total output tokens
- latest response timestamp
- provider breakdown
- per-response rows in verbose mode

## What prompt cache is not

Prompt cache is not a substitute for:

- `memory.working`
- long-term memory
- explicit retrieval and promotion

Use prompt cache to optimize repeated prompt prefixes.
Use memory to store facts, state, and durable knowledge.
