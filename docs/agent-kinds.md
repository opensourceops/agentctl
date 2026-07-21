# Agent Kinds

`agentctl` currently supports exactly two `agents.<name>.kind` values:

- `builtin.heuristic`
- `openai.responses`

Those are enforced by the playbook schema and the runtime registry.

## Shared agent fields

All agent kinds support these shared fields:

```yaml
agents:
  name:
    kind: builtin.heuristic | openai.responses
    description: optional string
    instructions: optional string
    instructionsFile: optional path
    vars: optional object
    promptCache: optional object
    maxTurns: optional integer
    profile: optional none|inspect|workspace_write|workspace_exec
    tools: optional array
```

Rules:

- exactly one of `instructions` or `instructionsFile` must be set
- `vars` are agent-level defaults
- task-scoped `tasks[].vars` override agent defaults at execution time
- `promptCache` is a runtime-owned optimization layer, not a memory store
- `tools` defines the tools the agent is allowed to attempt to call
- `profile` applies the agent tool policy profile

## `builtin.heuristic`

Use `builtin.heuristic` when you want:

- deterministic local behavior
- no external model provider
- simple bounded tool sequences
- reliable smoke tests and examples

Behavior:

- if no tools are configured, the agent returns the rendered instructions as `finalText`
- if tools are configured, it calls them in listed order, one per turn
- after collecting observations, it finishes using the last observation

Example:

```yaml
agents:
  reviewer:
    kind: builtin.heuristic
    instructionsFile: ./prompts/review.md
    vars:
      severity: medium
    maxTurns: 4
    profile: workspace_exec
    tools:
      - tool: builtin/read
      - tool: builtin/find
```

Kind-specific expectations:

- `provider` is not used
- `promptCache` is not supported
- `model` is not used
- `temperature` is not used
- `maxOutputTokens` is not used
- `reasoningEffort` is not used
- `organization`, `project`, `baseUrl`, `endpoint`, `apiVersion`, `deployment` are not used

Recommended use cases:

- local examples
- deterministic regressions
- pack and tool integration tests
- environments where no provider auth should be required

## `openai.responses`

Use `openai.responses` when you want:

- provider-backed autonomous behavior
- model-chosen tool use
- multi-turn tool execution with final synthesis
- OpenAI Responses API or Azure OpenAI Responses API

Required fields:

```yaml
agents:
  auditor:
    kind: openai.responses
    provider: openai | azure-openai-responses
    model: gpt-5
    instructionsFile: ./prompts/audit.md
```

Supported provider-related fields:

- `provider`
- `model`
- `promptCache`
- `baseUrl`
- `organization`
- `project`
- `endpoint`
- `apiVersion`
- `deployment`
- `temperature`
- `maxOutputTokens`
- `reasoningEffort`

Provider notes:

- `provider: openai`
  - uses the OpenAI Responses API
  - auth comes from `--api-key`, `~/.agentctl/auth.json`, or `OPENAI_API_KEY`
  - prompt cache is supported here
- `provider: azure-openai-responses`
  - uses the Azure OpenAI Responses API path
  - typically needs `endpoint` and `apiVersion`
  - prompt cache is not currently supported here

Behavior:

- renders instructions after merging agent default vars and task-scoped vars
- sends the task input and rendered instructions to the provider
- optionally sends `prompt_cache_key` and retention metadata when prompt cache is enabled
- advertises configured tools to the model
- executes requested tools through the same runtime policy, tracing, and checkpoint flow
- continues with `previous_response_id` / tool outputs until the model finishes or `maxTurns` is exceeded

Prompt cache shape:

```yaml
promptCache:
  enabled: true
  force: true
  retention: in_memory | 24h
  keyScope: agent | run | playbook | provider | custom
  shareMode: isolated | group
  group: optional group name
  keyTemplate: optional custom template when keyScope=custom
```

Use prompt cache when:

- the agent has a stable prompt prefix
- the same provider/model path will be reused across turns or tasks
- you want lower repeated prompt cost and latency

Do not use prompt cache as if it were memory:

- agents do not read or write cache contents
- cache reuse is provider-native and opaque
- stats are available through `agentctl prompt-cache stats`
- custom OpenAI-compatible base URLs require `force: true` or prompt cache stays disabled

Example:

```yaml
agents:
  ops_auditor:
    kind: openai.responses
    provider: openai
    model: gpt-5
    instructionsFile: ./prompts/audit.md
    maxTurns: 6
    profile: workspace_exec
    tools:
      - tool: builtin/find
      - tool: builtin/read
      - tool: builtin/write
```

Recommended use cases:

- model-backed audits
- tool-using autonomous runs
- cases where deterministic heuristics are too limited

## Validation and runtime behavior

Validation happens in two layers:

1. schema validation
   - only `builtin.heuristic` and `openai.responses` are accepted
2. runtime model registry
   - the runtime must have a registered model implementation for the selected kind

So a playbook with an unsupported kind fails early, and a runtime missing a model implementation fails with a concrete registry error.

## How to choose

Use `builtin.heuristic` when:

- you want repeatable local behavior
- the agent should just run a fixed tool sequence
- you are testing framework mechanics rather than model quality

Use `openai.responses` when:

- the model must decide which tools to use
- the final answer must be synthesized from multiple tool observations
- you are building real provider-backed autonomy rather than a deterministic harness
