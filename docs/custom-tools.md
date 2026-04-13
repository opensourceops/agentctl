# Custom Pack Tools

This document defines how packs can expose custom tools to `agentctl` agents.

## Scope

`agentctl` has three tool sources:

- builtin tools
- pack-defined tools
- external tools wrapped by packs

Builtin tools remain first-party and runtime-native.

Pack-defined tools are the extension point for:

- scripts shipped inside the pack
- existing host commands already installed on the machine
- future remote wrappers, if the pack chooses to wrap them behind a process or service boundary

## Current Runtime Contract

The current supported custom tool kind is:

- `pack.process`

This is a process-backed module definition that can be used:

- directly in tasks
- indirectly as an agent tool by referencing the module in `tools:`

The runtime contract is the same in both cases:

- inputs are resolved through the normal templating path
- policy is enforced before execution
- execution is traced and audited
- task checkpoints still govern retry, resume, and replay behavior

## Why one process-backed kind first

This is the smallest correct abstraction.

It covers both:

- “I ship a script in my pack”
- “I want to wrap an existing host command”

without introducing a second tool runtime or speculative plugin model.

## Module Shape

Example wrapping an existing host command:

```yaml
modules:
  node_version:
    kind: pack.process
    command: node
    args:
      - --version
    runtime:
      requires:
        - command: node
          version: ">=22"
    policy:
      label: node.version
      capability: observe
      risk: low
```

Example running a script shipped inside the pack:

```yaml
modules:
  fixture_audit:
    kind: pack.process
    command: node
    args:
      - ./tools/audit-service.mjs
      - ./fixtures/service
    cwd: .
    runtime:
      requires:
        - command: node
          version: ">=22"
    policy:
      label: fixture.audit
      capability: observe
      risk: low
```

## Fields

- `command`
  - required
  - executable name or path
- `args`
  - optional string array
  - passed directly to the executable
- `cwd`
  - optional working directory
  - resolved relative to the pack file when declared in a pack
- `env`
  - optional string-to-string environment overrides
- `with`
  - optional default input values
- `runtime.requires`
  - optional list of runtime requirements
- `policy`
  - optional tool metadata used by the policy engine

## Pack-relative resolution

When a `pack.process` module is loaded from a pack file:

- path-like `command` values such as `./bin/tool` are resolved relative to the pack file
- `cwd` is resolved relative to the pack file
- plain executable names such as `node`, `python3`, or `terraform` are left unchanged

This lets a pack support both shipped tools and existing host tools.

## Runtime Requirements

`runtime.requires` is the preflight declaration.

It is checked before `run`, `resume`, and `replay` start executing.

Current requirement fields:

- `command`
  - required executable to find
- `version`
  - optional minimum version constraint
  - currently supports `>=x.y.z`
- `versionArgs`
  - optional custom version command arguments
  - default is `--version`

Example:

```yaml
runtime:
  requires:
    - command: node
      version: ">=22"
    - command: terraform
      version: ">=1.8.0"
```

Failure behavior:

- if a required executable is missing, the run fails before a run record is created
- if a version constraint is not satisfied, the run fails before execution starts

That is intentional. Missing runtime dependencies are environment errors, not in-run task failures.

## Agent Usage

Agents do not automatically get custom tools.

The same rule applies as with builtin tools:

- define the module in the pack or playbook
- expose it in the agent’s `tools:` list
- let policy/profile decide whether the agent may call it

Example:

```yaml
agents:
  auditor:
    kind: builtin.heuristic
    profile: inspect
    tools:
      - tool: custom/node_version
      - tool: custom/fixture_audit
```

## Policy Model

Custom tools do not bypass policy.

`policy` metadata on the module defines:

- `label`
- `capability`
  - `internal`
  - `observe`
  - `mutate`
  - `act`
- `risk`
  - `low`
  - `medium`
  - `high`

Defaults for `pack.process`:

- `capability: act`
- `risk: high`
- `label: basename(command)`

That means pack tools are treated conservatively unless the pack author narrows them deliberately.

## What this supports today

- wrapping existing installed host commands
- shipping custom scripts inside a pack
- using custom tools from tasks
- using custom tools from agents
- preflighting declared runtime dependencies
- preserving checkpoint/replay behavior for custom-tool tasks

## What this does not support yet

- container-backed custom tools
- HTTP-backed custom tool definitions
- custom output schema validation
- automatic runtime installation

Those can be added later without changing the current process-tool contract.
