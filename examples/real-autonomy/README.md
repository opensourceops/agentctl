# Real Autonomy Example

This example exercises a real model-backed autonomous run with `agentctl`.

The agent:

- inspects a small service fixture under `./fixtures/service`
- decides which workspace read tools to use
- returns the final report Markdown
- has that report persisted to `./artifacts/ops-readiness-report.md` by a deterministic task
- passes a deterministic verification step that checks for required findings and concrete evidence from the fixture files

## Requirements

- `OPENAI_API_KEY` set in the environment, or `--api-key` passed to the CLI
- network access to the OpenAI Responses API

## Run

```bash
cd /Users/ompragash/Git/agentctl
npm link
agentctl run examples/real-autonomy/mission.playbook.yaml --db .runtime/real-autonomy.db
```

Or with a one-shot override:

```bash
cd /Users/ompragash/Git/agentctl
npm link
agentctl run examples/real-autonomy/mission.playbook.yaml --db .runtime/real-autonomy.db --api-key "$OPENAI_API_KEY"
```

## Expected outcome

The run should:

- complete with `status: "succeeded"`
- create `examples/real-autonomy/artifacts/ops-readiness-report.md`
- mention the missing backup restore drill
- mention the missing documented on-call escalation policy
- cite `README.md` and `docs/runbook.md`
- quote the concrete evidence lines from those files

The final verification task uses `set -eu`, so the run fails immediately if the artifact is missing, any required finding is absent, or the report is not grounded in the fixture evidence.
