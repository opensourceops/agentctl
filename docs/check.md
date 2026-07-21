# `agentctl check`

`agentctl check` validates a playbook without running it.

```bash
agentctl check examples/prompt-file-vars/mission.playbook.yaml
agentctl check examples/prompt-file-vars/mission.playbook.yaml --output json
```

## What it validates

`check` covers these phases:

1. YAML syntax
2. playbook schema
3. pack schema
4. prompt-file loading
5. template-reference sanity
6. graph/reference compilation

## Error reporting

For YAML syntax errors, `agentctl check` reports:

- file
- phase: `yaml_syntax`
- line
- column
- parser detail

For schema, prompt-file, template, and compile errors, it reports:

- file
- phase
- optional field path
- error detail

## Output contract

Success:

```yaml
type: check
ok: true
playbook: /absolute/path/to/playbook.yaml
packs: []
compiled: true
```

Failure:

```yaml
type: check
ok: false
playbook: /absolute/path/to/playbook.yaml
packs: []
diagnostics:
  - file: /absolute/path/to/playbook.yaml
    phase: yaml_syntax
    line: 12
    column: 7
    detail: Nested mappings are not allowed in compact mappings
```

## Current template checks

`check` validates obvious prompt-template mistakes before execution:

- prompt references a merged var that is not defined by agent defaults plus task-scoped vars for that specific task invocation
- prompt or var references `tasks.<id>` where the task does not exist
- task `with` references a merged var that is not defined for that task

That validation is task-aware. If the same agent is used by two tasks with different `vars`, `check` validates each task separately.

It does not attempt full runtime-data validation. Dynamic values may still fail at execution time if the required task output or working-memory key is absent when the agent runs.
