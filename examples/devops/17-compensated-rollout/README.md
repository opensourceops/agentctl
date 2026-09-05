# 17. Compensated rollout

Fail after a local rollout, plan and execute its explicit inverse, and inspect linked reconciliation evidence.

## Run

From the framework checkout:

```sh
cargo build -p agentctl-cli --locked
python3 examples/devops/run.py --agentctl target/debug/agentctl --only 17 --keep --report /tmp/agentctl-devops-17.json
```

The runner copies this directory and the checked-in common helper into a new temporary directory and invokes the real CLI from a different clean directory with an explicit workspace. The checked-in workflow uses the JSON-compatible subset of YAML (`agentctl.dev/v1`). `--keep` prints the retained location; the JSON report includes it. It records every CLI command, result envelope, inspection and replay in `evidence/`.

## Prerequisites, authority and limits

Python 3.10+ and a built agentctl binary are required. No credentials; no live model is needed.

The workflow grants workspace access, writes only under `artifacts`, and explicitly allows `python3` for the reviewed helper where required. Host process execution is **not a security sandbox**: the trusted helper can spawn its documented local Git/Python subprocesses. No production cluster, cloud account, package registry or external deployment is accessed. Fixtures contain no secrets. Network access is absent except explicit api.openai.com in a live variant; the HTTP demonstration is a runner-owned loopback service.

The DSL carries request, turn, token, task, wall-time, process-output and artifact bounds. The runner adds subprocess deadlines. Ordinary variable files contain configuration only, not secrets or policy grants.

## Expected artifacts and semantic assertions

The runner validates the case-specific structured report and its source-derived fields.

- `artifacts/service.json`
- `evidence/inspect.json`

Deterministic execution status is recorded by the suite report, not inferred from static checking. Live model output is checked semantically, never by exact prose equality.

## Failure, recovery and cleanup

The runner requires initial failure, inspects compensate --plan, executes compensate, checks version=restored, and asserts a compensated reconciliation links the source effect. Compensation is a best-effort inverse, not transactional rollback.

The runner exercises a denied process or write in a separate workspace and verifies there is no forbidden output. Replay is run with provider credential removed and must preserve effect count and artifact bytes. Remove only the printed temporary workspace when finished; without `--keep`, the runner cleans it automatically. Disposable services and containers are stopped even if an assertion fails.
