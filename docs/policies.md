# Policies

`agentctl` policies are runtime guardrails for tool execution.

They do not decide which tools an agent sees. Tool declaration and agent profiles handle that. Policies decide whether a requested tool call is allowed, denied, or requires approval.

Subprocess-backed tools are a special case. `builtin.shell.exec` and `pack.process` can launch arbitrary commands, so path checks on `cwd` alone are not enough to make them safe. When an agent tries to call one of those tools, `agentctl` requires approval even if `approvalMode: never`.

## Supported policy fields

Playbooks currently support these policy fields:

```yaml
policy:
  workspaceRoot: ./relative-or-absolute-path
  writableRoots:
    - ./path-a
    - ./path-b
  approvalMode: never | on-mutate | on-act | always
```

That is the full supported policy surface today.

## Field behavior

### `workspaceRoot`

`workspaceRoot` defines the root boundary for path-based observation and shell working directories.

The runtime canonicalizes it at startup.

It is used for:

- resolving relative file paths
- preventing observe tools from escaping the allowed workspace
- preventing subprocess tools from using a `cwd` outside the workspace root
- preventing symlink traversal from escaping the allowed workspace

### `writableRoots`

`writableRoots` defines where mutate-capability tools may write.

The runtime canonicalizes every entry at startup.

It is used for:

- `builtin.write`
- `builtin.edit`
- any other tool whose capability is `mutate`

If a target path is outside all writable roots, the tool call is denied.

### `approvalMode`

`approvalMode` defines when a tool call should return `require_approval` instead of `allow`.

Supported values:

- `never`
  - no approval requirement from policy
- `on-mutate`
  - approval required for `mutate` and `act`
- `on-act`
  - approval required for `act` only
- `always`
  - approval required for every non-`internal` capability

`internal` capability never requires approval from this policy rule.

When approval is required, `agentctl` does not fail the run immediately. The run is paused, the blocked task moves to `waiting_approval`, and an approval record is persisted in the runtime DB.

You can then:

- inspect the pending request with `agentctl approvals list` or `agentctl approvals show <approval-id>`
- resolve it with `agentctl approvals approve <approval-id>` or `agentctl approvals reject <approval-id>`
- continue the run with `agentctl resume <playbook.yaml> <run-id>`

In interactive YAML TTY mode, `agentctl run`, `resume`, and `replay` can prompt inline for approval and continue automatically.

## Decision flow

For a tool call, `agentctl` evaluates policy in this order:

1. agent profile capability check, when the origin is an agent tool call
2. path guardrails:
   - any tool input `cwd` must stay inside `workspaceRoot`
   - observe tool `path` must stay inside `workspaceRoot`
   - mutate tool `path` must stay inside one of `writableRoots`
3. subprocess guardrail:
   - agent-origin `builtin.shell.exec` and `pack.process` calls require approval
4. approval mode
5. final decision:
   - `allow`
   - `deny`
   - `require_approval`

## Current path rules by capability

### Observe tools

If the tool input includes a string `path`, the resolved canonical target path must remain inside `workspaceRoot`.

This applies to tools such as:

- `builtin.read`
- `builtin.find`
- `builtin.grep`
- `builtin.ls`
- any custom or remote tool classified as `observe` and taking a `path`

### Mutate tools

If the tool input includes a string `path`, the resolved canonical target path must remain inside one of the configured `writableRoots`.

This applies to tools such as:

- `builtin.write`
- `builtin.edit`
- `builtin.long_term_memory.write` is `mutate` by capability, but it is not path-based, so writable-root checks do not apply to it

### Act tools

For subprocess-backed tools, the runtime validates the `cwd` when one is supplied:

- the resolved `cwd` must remain inside `workspaceRoot`

For agent-origin subprocess calls, the runtime also requires approval regardless of `approvalMode`, because `cwd` checks do not restrict what the command itself can do.

Other `act` tools are governed mainly by profile and approval mode unless they also expose a `path` input and use the generic path checks.

## What policies do not currently cover

The current policy engine does not yet implement:

- network allowlists
- environment variable allowlists
- per-tool explicit deny/allow lists in YAML
- separate MCP/A2A-specific auth policies
- sandbox mode policy

Those may be added later, but they are not part of the current supported surface.

## How policy interacts with profiles

Profiles and policies are separate:

- profiles say whether an agent is allowed to use a capability at all
- policies say whether the specific call is safe in the current workspace and whether approval is required

So this is possible:

- profile allows `mutate`
- policy still denies the write because the target path is outside `writableRoots`

## Examples

Read-only workspace policy:

```yaml
policy:
  workspaceRoot: .
  writableRoots: []
  approvalMode: never
```

This allows observe tools inside the repo but denies path-based mutations.

Editable workspace policy:

```yaml
policy:
  workspaceRoot: .
  writableRoots:
    - .
  approvalMode: on-act
```

This allows writes inside the repo, but `act` tools still require approval.

Restricted writable subtree:

```yaml
policy:
  workspaceRoot: .
  writableRoots:
    - ./artifacts
    - ./reports
  approvalMode: on-mutate
```

This allows writes only under `artifacts` and `reports`, and requires approval for both mutate and act tool calls.

## Failure behavior

Typical denial reasons are:

- `path "/abs/path" escapes workspaceRoot`
- `path "/abs/path" is not inside writableRoots`
- `bash cwd "/abs/path" escapes workspaceRoot`
- `custom-tool cwd "/abs/path" escapes workspaceRoot`
- `... requires approval under approvalMode=...`
- `... launches a subprocess and requires approval`

Denials are deliberate hard failures so policy mistakes are visible immediately.

Approval requirements are not denials. They pause the run until the approval is resolved.
