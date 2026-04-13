# Dataflow Example

This example proves YAML step-to-step output passing in two forms:

- scalar output propagation
- structured JSON object propagation

The flow is:

1. `produce` assigns a scalar and a nested object.
2. `consume_scalar` reads the scalar from `tasks.produce.output`.
3. `consume_object` reads the nested object from `tasks.produce.output`.
4. `assert_scalar` and `assert_object` verify both values survived unchanged.

Run it with:

```bash
agentctl run examples/dataflow/mission.playbook.yaml --db .runtime/dataflow.db
```

Successful output proves:

- task output templating works
- nested arrays and objects are preserved across task boundaries
- deterministic assertions can validate the handoff without custom code
