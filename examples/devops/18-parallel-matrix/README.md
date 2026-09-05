# 18. Parallel service matrix

Run a bounded two-service/two-check matrix and verify stable ordered aggregation.

## Run

From the framework checkout:

```sh
cargo build -p agentctl-cli --locked
python3 examples/devops/run.py --agentctl target/debug/agentctl --only 18 --keep --report /tmp/agentctl-devops-18.json
```

The runner copies this directory and the checked-in common helper into a new temporary directory and invokes the real CLI from a different clean directory with an explicit workspace. The checked-in workflow uses the JSON-compatible subset of YAML (`agentctl.dev/v1`). `--keep` prints the retained location; the JSON report includes it. It records every CLI command, result envelope, inspection and replay in `evidence/`.

## Prerequisites, authority and limits

Python 3.10+ and a built agentctl binary are required. No credentials; no live model is needed.

The workflow grants workspace access, writes only under `artifacts`, and explicitly allows `python3` for the reviewed helper where required. Host process execution is **not a security sandbox**: the trusted helper can spawn its documented local Git/Python subprocesses. No production cluster, cloud account, package registry or external deployment is accessed. Fixtures contain no secrets. Network access is absent except explicit api.openai.com in a live variant; the HTTP demonstration is a runner-owned loopback service.

The DSL carries request, turn, token, task, wall-time, process-output and artifact bounds. The runner adds subprocess deadlines. Ordinary variable files contain configuration only, not secrets or policy grants.

## Expected artifacts and semantic assertions

The runner validates the case-specific structured report and its source-derived fields.

- `evidence/inspect.json`
- `evidence/run.json`

Deterministic execution status is recorded by the suite report, not inferred from static checking. Live model output is checked semantically, never by exact prose equality.

## Failure, recovery and cleanup

Replay with the runner verifies that recorded execution creates zero fresh effects. Fix bad fixture data in a new workspace before a new run.

The runner exercises a denied process or write in a separate workspace and verifies there is no forbidden output. Replay is run with provider credential removed and must preserve effect count and artifact bytes. Remove only the printed temporary workspace when finished; without `--keep`, the runner cleans it automatically. Disposable services and containers are stopped even if an assertion fails.
