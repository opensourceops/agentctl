# Agent Profiles

`agentctl` profiles control which tool capabilities an agent is allowed to use when it calls tools.

Profiles apply only to agent tool calls. They do not change what a normal module task may do when the playbook itself directly uses a module.

## Supported profiles

`agentctl` currently supports exactly these profile names:

- `none`
- `inspect`
- `workspace_write`
- `workspace_exec`

## Capability model

Each tool has one of these capabilities:

- `internal`
- `observe`
- `mutate`
- `act`

Current built-in capability mapping:

- `internal`
  - `builtin.assign`
  - `builtin.assert`
  - `builtin.memory.read`
  - `builtin.memory.write`
  - `builtin.long_term_memory.retrieve`
- `observe`
  - `builtin.read`
  - `builtin.grep`
  - `builtin.find`
  - `builtin.ls`
  - `builtin.long_term_memory.search`
- `mutate`
  - `builtin.write`
  - `builtin.edit`
  - `builtin.long_term_memory.write`
- `act`
  - `builtin.shell.exec`
  - `pack.process` by default, unless the pack overrides the capability in its module `policy`

Remote MCP and A2A tools are also assigned a capability through their policy spec before profile checks run.

## Profile matrix

Profile behavior is a straight capability allowlist:

- `none`
  - allows: `internal`
  - denies: `observe`, `mutate`, `act`
- `inspect`
  - allows: `internal`, `observe`
  - denies: `mutate`, `act`
- `workspace_write`
  - allows: `internal`, `observe`, `mutate`
  - denies: `act`
- `workspace_exec`
  - allows: `internal`, `observe`, `mutate`, `act`
  - denies: nothing in the current capability model

## How profiles are selected

You can set a default profile for agents:

```yaml
defaults:
  agentProfile: inspect
```

You can also set a profile per agent:

```yaml
agents:
  reviewer:
    kind: builtin.heuristic
    profile: workspace_write
```

Agent-level `profile` overrides `defaults.agentProfile`.

If no profile is set anywhere, the runtime uses `none`.

## What profiles do not do

Profiles do not:

- grant tools automatically
- bypass policy checks
- bypass path restrictions
- bypass approval mode

An agent must still have the tool explicitly listed under `tools:`.

The runtime then applies profile checks after tool declaration and before execution.

## Recommended usage

Use `none` when:

- the agent should only do internal bookkeeping
- the agent should not observe or mutate the workspace

Use `inspect` when:

- the agent should read/search/list only
- you want a safe analysis-only agent

Use `workspace_write` when:

- the agent may inspect and edit files
- shell execution should still be blocked

Use `workspace_exec` when:

- the agent needs to run commands
- the agent needs the full workspace tool surface
- you accept the higher risk of `act` tools

## Examples

Read-only audit agent:

```yaml
defaults:
  agentProfile: inspect

agents:
  auditor:
    kind: openai.responses
    provider: openai
    model: gpt-5
    instructionsFile: ./prompts/audit.md
    tools:
      - tool: builtin/find
      - tool: builtin/read
```

Editable but non-exec agent:

```yaml
agents:
  fixer:
    kind: openai.responses
    provider: openai
    model: gpt-5
    profile: workspace_write
    instructionsFile: ./prompts/fix.md
    tools:
      - tool: builtin/read
      - tool: builtin/edit
      - tool: builtin/write
```

Command-running agent:

```yaml
agents:
  builder:
    kind: openai.responses
    provider: openai
    model: gpt-5
    profile: workspace_exec
    instructionsFile: ./prompts/build.md
    tools:
      - tool: builtin/read
      - tool: builtin/bash
```

## Failure behavior

If an agent tool call violates the profile, the runtime denies it with a concrete error such as:

```text
Agent profile "inspect" does not allow write
```

That denial happens before the tool executes.
