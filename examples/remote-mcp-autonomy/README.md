# Remote MCP Autonomy Example

This example proves that `agentctl` can run autonomously across a real remote MCP boundary, not just local built-in tools.

The flow is:

- a standalone MCP HTTP server listens on `http://127.0.0.1:43127/mcp`
- the agent calls `mcp:auditor/audit_service`
- the remote server reads the fixture and returns a report through MCP
- a deterministic task persists the report to `./artifacts/remote-mcp-report.md`
- a deterministic verification task checks the required findings

## Run

Start the MCP server in one terminal:

```bash
cd /Users/ompragash/Git/agentctl
npm link
node examples/remote-mcp-autonomy/mock-mcp-server.mjs
```

Run the playbook in another terminal:

```bash
cd /Users/ompragash/Git/agentctl
npm link
agentctl run examples/remote-mcp-autonomy/mission.playbook.yaml --db .runtime/remote-mcp-autonomy.db
```

## Expected outcome

The run should:

- complete with `status: "succeeded"`
- create `examples/remote-mcp-autonomy/artifacts/remote-mcp-report.md`
- mention the missing rollback validation drill
- mention the missing incident communication owner

The verification task uses `set -eu`, so the run fails immediately if the artifact is missing or the report omits either finding.
