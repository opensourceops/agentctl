# Selective repair with OpenAI

This example is an opt-in live acceptance scenario for `agentctl repair`.

The source workflow runs `analyze` successfully, then deliberately exhausts
`publish` after its first model-selected read-only tool call. The repaired
workflow changes only `publisher.maxTurns`. Repair from `publish` must reuse the
durable structured `analyze` output without another provider or tool dispatch,
execute deterministic verification, and write
`artifacts/selective-repair-result.txt`.

```text
agentctl run source.workflow.yaml --workspace . --db repair.db
agentctl repair repaired.workflow.yaml SOURCE_RUN_ID --from publish --plan --workspace . --db repair.db
agentctl repair repaired.workflow.yaml SOURCE_RUN_ID --from publish --workspace . --db repair.db
agentctl inspect REPAIR_RUN_ID --db repair.db --output json
env -u OPENAI_API_KEY agentctl replay REPAIR_RUN_ID --db repair.db --output json
```

The first and third commands perform live OpenAI requests. Planning, inspection,
and recorded replay do not. `OPENAI_API_KEY` is read only from the environment.

Run every credential-free example check with `cargo xtask examples-verify`.
The bounded all-OpenAI gate is
`cargo xtask examples-verify-live-openai`; it retains only sanitized metadata in
the ignored `.release-evidence/` directory.
