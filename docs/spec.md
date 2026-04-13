# agentctl v0.1

## YAML schema

### Playbook

```yaml
playbook: <name>
version: 0.1.0
description: <optional text>
packs:
  - ./relative-pack.yaml
inputs:
  key: value
defaults:
  agentProfile: none | inspect | workspace_write | workspace_exec
policy:
  workspaceRoot: <optional relative or absolute path>
  writableRoots:
    - <optional relative or absolute path>
  approvalMode: never | on-mutate | on-act | always
mcpServers:
  server_name:
    description: <optional text>
    url: <optional https endpoint for MCP Streamable HTTP>
    headers:
      X-Header: value
    bearerTokenEnv: OPTIONAL_ENV_NAME
a2aAgents:
  agent_name:
    description: <optional text>
    url: <optional direct A2A RPC endpoint>
    cardUrl: <optional agent card URL>
    headers:
      X-Header: value
    bearerTokenEnv: OPTIONAL_ENV_NAME
modules:
  local/name:
    kind: builtin.assign | builtin.assert | builtin.shell.exec | builtin.read | builtin.write | builtin.edit | builtin.grep | builtin.find | builtin.ls
    with: {}
agents:
  local/name:
    kind: builtin.heuristic | openai.responses
    instructions: <text>
    maxTurns: 4
    profile: none | inspect | workspace_write | workspace_exec
    provider: openai
    model: gpt-5-mini
    baseUrl: <optional provider base URL override>
    organization: <optional OpenAI organization override>
    project: <optional OpenAI project override>
    endpoint: <optional Azure OpenAI endpoint override>
    apiVersion: <optional Azure OpenAI API version override>
    deployment: <optional Azure OpenAI deployment override>
    temperature: 0
    maxOutputTokens: 4096
    reasoningEffort: minimal | low | medium | high
    tools:
      - tool: builtin/bash | builtin/read | builtin/write | builtin/edit | builtin/grep | builtin/find | builtin/ls | mcp:<server>/<tool> | a2a:<agent> | <module ref>
        name: optional-name
        with: {}
tasks:
  - id: unique_id
    uses: module:<ref> | agent:<ref>
    needs: [other_task]
    with: {}
    retry:
      maxAttempts: 1
      backoffMs: 0
```

### Pack

```yaml
pack: namespace
version: 0.1.0
modules: {}
agents: {}
```

Pack members are imported as `<pack>/<member>`.

## Runtime object model

- `CompiledPlaybook`: validated task graph plus resolved module/agent registries.
- `CompiledPlaybook.defaults`: default agent profile for agent tool authorization.
- `CompiledPlaybook.policy`: resolved workspace guardrails and approval mode.
- `CompiledPlaybook.mcpServers`: declared MCP servers plus optional remote transport configuration.
- `CompiledPlaybook.a2aAgents`: declared A2A peers plus optional remote transport configuration.
- `ModelRegistry`: resolves provider model configuration and auth for provider-backed agents.
- `AuthStorage`: resolves runtime overrides, persisted credentials, and environment variables.
- `RuntimeSnapshot`: checkpointable execution state containing:
  - `inputs`
  - `vars`
  - `tasks`
  - `agents`
- `TaskState`: `pending | running | succeeded | failed`, attempts, output, error.
- `AgentSessionState`: current attempt, resolved input, and persisted turn history.
- `CheckpointRecord`: immutable snapshot for replay and resume.
- `RunRecord`: mutable head pointer to the latest snapshot.

## SQLite schema

Tables:

- `runs`: latest execution head for each run
- `checkpoints`: immutable snapshots keyed by `(run_id, seq)`
- `task_attempts`: per-task attempt history
- `agent_turns`: persisted turn-by-turn agent decisions and observations
- `audit_events`: user-facing operational events
- `trace_spans`: internal span model compatible with OpenTelemetry export

The store uses WAL mode for durability and concurrent readers.

## Task and agent execution semantics

1. Compile the playbook into a DAG.
2. Create an initial snapshot with all tasks `pending`.
3. Select the next runnable task when all dependencies succeeded.
4. Checkpoint before the first execution of a task attempt.
5. Execute:
   - module task: deterministic module executor or side-effecting shell module
   - agent task: bounded loop with persisted turn history and policy-gated tool execution
     - `builtin.heuristic`: deterministic local heuristic model
     - `openai.responses`: provider-backed loop using the OpenAI Responses API and persisted `previous_response_id`
   - provider tools:
     - local builtin/module tools
     - MCP tools via `mcp:<server>/<tool>`, using either injected transports or remote Streamable HTTP sessions
     - A2A delegation via `a2a:<agent>`, using either injected transports or remote HTTP discovery plus task polling
6. Checkpoint after every agent turn and after task completion/failure.
7. On resume:
   - completed tasks remain completed
   - interrupted module tasks are reset to `pending`
   - interrupted agent tasks continue from persisted turn history
8. On replay: create a new run seeded from an earlier checkpoint snapshot.

## Built-in tool profiles

- `none`: internal-only tools such as `builtin.assign` and `builtin.assert`
- `inspect`: `builtin/read`, `builtin/grep`, `builtin/find`, `builtin/ls`
- `workspace_write`: `inspect` plus `builtin/write` and `builtin/edit`
- `workspace_exec`: `workspace_write` plus `builtin/bash`

Built-in tools are not auto-injected into agents. Agents must still list the tools they intend to use. Profiles decide whether those tool calls are allowed.

## MCP and A2A

- MCP supports:
  - injected in-process transports for tests or embedded runtimes
  - remote Streamable HTTP endpoints declared with `mcpServers.<name>.url`
- Remote MCP behavior:
  - sends `initialize`
  - sends `notifications/initialized`
  - caches `MCP-Session-Id`
  - reuses `MCP-Protocol-Version`
  - lists tools before the first remote call
- Remote MCP tools default to high-risk `act` capability unless the server advertises read-only hints.
- A2A supports:
  - injected in-process peer transports
  - remote endpoints declared directly with `a2aAgents.<name>.url`
  - remote discovery from `a2aAgents.<name>.cardUrl`
- Remote A2A behavior:
  - fetches the agent card when needed
  - sends `message/send`
  - falls back to `tasks/send` for older peers
  - polls `tasks/get` until a terminal task state is reached
- A2A delegated calls produce task/context identifiers and return structured task output into the calling agent turn.

## Provider auth resolution

- runtime CLI override via `--api-key`
- persisted provider key in `~/.agentctl/auth.json`
- provider environment variable lookup
- preflight inspection via `agentctl auth check [playbook.yaml] [--provider name]`

`auth check` inspects either:

- the provider-backed agents referenced by a playbook
- or the explicit `--provider` value when no playbook is supplied

The command emits JSON and exits nonzero when any inspected provider is missing auth, so deployments can gate execution before `run`.

The current live provider implementation is `openai.responses`, with two provider configurations behind the same agent kind:

- `openai`
  - API key plus optional organization, project, and base URL
- `azure-openai-responses`
  - API key plus Azure endpoint and API version, using the official `AzureOpenAI` client path

Stored credentials can be legacy strings or structured `api_key` objects with provider metadata such as `organization`, `project`, `endpoint`, and `apiVersion`. The auth/model registry is intentionally generic so additional providers can be added without changing the playbook runtime contract.

## Pack packaging format

`packs` are distribution units. They package reusable:

- modules
- agents
- policies

The v0.1 runtime resolves pack files directly from local paths listed in a playbook. A future registry can keep the same manifest contract.
