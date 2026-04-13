# Memory Flow

This example proves the two memory layers added to `agentctl`:

- `working` memory is checkpointed inside the runtime DB for the current run
- `longTerm` memory is stored separately and survives across runs

Run it with:

```bash
agentctl run examples/memory-flow/mission.playbook.yaml --db .runtime/memory-flow.db
```

What it does:

1. seeds working memory with `service=checkout`
2. writes `finding=restore-drill-missing` into working memory
3. reads the working-memory value back
4. persists the same fact into long-term memory
5. searches the long-term store by exact key
6. asserts the search returned exactly one match
7. retrieves that long-term entry back into working memory as `memory.working.recalled`
8. verifies the promoted value matches the original finding

Artifacts/state to inspect after the run:

- runtime DB: `~/.agentctl/runtime/runtime.db` unless `--db` overrides it
- example runtime DB from the command above: `.runtime/memory-flow.db`
- long-term memory DB for this example: `examples/memory-flow/state/long-term.db`

Expected outcome:

- the run completes with `status: "succeeded"`
- `tasks.read_long_term.output.matchCount` is `1`
- `memory.working.recalled` is `restore-drill-missing`
