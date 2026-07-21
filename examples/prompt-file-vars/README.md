# Prompt File Vars Example

This example proves:

- agent instructions can come from a file with `instructionsFile`
- task-scoped `vars` are the primary invocation surface for both agents and modules
- agent-level `vars` act as reusable defaults
- prompt placeholders can use bare names like `{{ service }}` or the alias form `{{ vars.severity }}`
- task vars can resolve dynamic values from prior task output at execution time

Run it with:

```bash
agentctl check examples/prompt-file-vars/mission.playbook.yaml
agentctl run examples/prompt-file-vars/mission.playbook.yaml --db .runtime/prompt-file-vars.db
```

Expected result:

- `check` succeeds
- `project` proves module task templating with:
  - `{{ service }}`
  - `{{ vars.finding }}`
- `review` renders the prompt file with:
  - `service: checkout`
  - `finding: restore-drill-missing`
  - `severity: medium`
- `verify` and `verify_project` pass deterministically
