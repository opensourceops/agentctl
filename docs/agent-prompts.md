# Agent Prompt Files and Task Vars

`agentctl` agents can define instructions in two ways:

- inline with `instructions`
- from disk with `instructionsFile`

Exactly one must be set.

## Inline instructions with agent defaults

```yaml
agents:
  audit:
    kind: builtin.heuristic
    instructions: |
      Inspect {{ service }} and report the finding.
    vars:
      severity: medium
```

## Prompt files with task-scoped vars

```yaml
agents:
  audit:
    kind: builtin.heuristic
    instructionsFile: ./prompts/audit.md
    vars:
      severity: medium

tasks:
  - id: audit_checkout
    uses: agent:audit
    vars:
      service: checkout
      finding: "{{ tasks.prepare.output.values.finding }}"
```

Prompt file:

```md
Service: {{ service }}
Finding: {{ finding }}
Severity: {{ vars.severity }}
```

## Resolution rules and precedence

Var precedence is:

1. task-scoped `tasks[].vars`
2. agent default `agents.<name>.vars`

That precedence applies to both:

- bare references such as `{{ service }}`
- alias references such as `{{ vars.service }}`

1. `instructionsFile` is resolved relative to the playbook or pack file that defines the agent.
2. The prompt file is loaded during playbook loading.
3. Agent-level `vars` are reusable defaults.
4. Task-level `vars` are the primary invocation-time values.
5. Task vars override agent defaults on key collision.
6. Var resolution happens at execution time, not parse time.
7. Prompt rendering happens after var resolution.

Merged vars are available in both forms:

- bare: `{{ service }}`
- namespaced alias: `{{ vars.service }}`

Bare vars only resolve from the merged var bag. They do not implicitly fall back to:

- `inputs`
- `memory`
- `tasks`
- `run`

Use explicit namespaces for those:

That means dynamic values from runtime state are supported:

- `{{ inputs.foo }}`
- `{{ tasks.prepare.output.values.finding }}`
- `{{ memory.working.finding }}`
- `{{ vars.service }}`

Task vars and agent default vars can depend on other vars in the merged bag:

```yaml
vars:
  service: checkout
  heading: "Service: {{ service }}"
```

`agentctl` resolves vars iteratively until all referenced values are available or resolution fails.

## Module tasks use the same vars model

Task vars are not limited to agents. Module tasks can use the same merged var bag:

```yaml
tasks:
  - id: project
    uses: module:builtin.assign
    vars:
      service: checkout
      finding: "{{ tasks.prepare.output.values.finding }}"
    with:
      values:
        rendered: "{{ service }}:{{ vars.finding }}"
```

## Failure behavior

`agentctl` fails hard when:

- both `instructions` and `instructionsFile` are set
- neither is set
- the prompt file does not exist
- a prompt references an undefined merged var such as `{{ finding }}`
- a prompt references `{{ vars.finding }}` with no matching merged var
- a task var or agent default var cannot be resolved at execution time

There is no silent empty-string substitution for prompt files.

## Recommended usage

- Use `instructionsFile` when the prompt is long, reused, or edited frequently.
- Put invocation-specific values under `tasks[].vars`.
- Use `agents.<name>.vars` only for reusable defaults.
- Prefer bare names like `{{ service }}` in prompts for readability.
- Use `{{ vars.service }}` when you want to make the var namespace explicit.
- Use explicit namespaces for runtime state such as `{{ inputs.service }}` and `{{ memory.working.finding }}`.
- Keep prompt templates simple. `agentctl` supports placeholder lookups, not a full template language.

See [examples/prompt-file-vars/README.md](../examples/prompt-file-vars/README.md) for a runnable example.
