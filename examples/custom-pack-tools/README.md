# Custom Pack Tools

This example shows two process-backed pack tools:

- `custom/node_version`: wraps an existing host command (`node --version`)
- `custom/fixture_audit`: runs a script shipped inside the pack

Both tools are exposed to an agent through the normal `tools:` block, and both are preflight-checked before the run starts.

Run:

```bash
agentctl run examples/custom-pack-tools/mission.playbook.yaml --db .runtime/custom-pack-tools.db
```

Expected behavior:

- the agent calls both tools in order
- the run succeeds without requiring any built-in write or shell tool access for the agent
- the report is persisted to `./artifacts/custom-pack-report.md`
- verification confirms the report mentions:
  - the detected Node.js version
  - the missing rollback owner
  - the missing restore drill
