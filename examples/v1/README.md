# Verified examples

The deterministic examples are exercised by `cargo xtask verify` and never require credentials.

- `hello.yaml`: deterministic hello world and declared output.
- `dataflow.yaml`: typed scalar/object templates.
- `condition.yaml`: safe `when` equality and skipping.
- `check-diff.yaml`: predictable file diff without mutation under `run --check --diff`.
- `approval.yaml`: durable approval-gated workspace mutation.
- `policy-denial.yaml`: an explicit tool-policy denial.
- `crash-resume.yaml`: effect-ledger write followed by observation; crash behavior is injected in runtime tests.
- `working-memory.yaml` and `long-term-memory.yaml`: separate memory lifecycles.
- `fake-provider.yaml`: deterministic model-provider path.
- `mcp.yaml` and `a2a.yaml`: local protocol fixtures, backed by the protocol crate's mock-server tests.
- `example.pack.yaml` and `reusable-pack.yaml`: integrity-pinned local pack and executed packed action.
- `secret-reference.yaml`: environment reference without inline secret material.
- `capability-failure.yaml`: a workflow expected to fail during compilation.

The `*-live.yaml` files are opt-in configuration examples. They are checked statically but are never executed by normal tests. Only the separately bounded `providers smoke-openai --live` command is used for release smoke testing.

Run the quickstart:

```console
agentctl check examples/v1/hello.yaml
agentctl plan examples/v1/hello.yaml
agentctl run examples/v1/hello.yaml --db .agentctl/quickstart.db --output json
```

Preview a write without performing it:

```console
agentctl run examples/v1/check-diff.yaml --check --diff --db .agentctl/check.db
```

Recorded replay and fresh re-execution are deliberately separate:

```console
agentctl replay RUN_ID --db .agentctl/quickstart.db
agentctl fork RUN_ID --db .agentctl/quickstart.db
```
