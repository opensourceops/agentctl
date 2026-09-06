# 17. Compensate a failed local rollout

**For:** SRE. **Level and evidence:** Advanced; local compensation contract demonstration.

Capture prior state, attempt a rollout and run an explicit best-effort inverse when health validation fails.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 17 --output ./example-17
```

Enter the extracted directory containing `setup.py`. You need Python 3.11 or newer. Create an isolated environment and install the pinned example dependencies:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python setup.py
```

On Windows, use `.venv\Scripts\python.exe` in place of `.venv/bin/python`. Setup records the selected interpreters and prepares `local.workflow.yaml` with a matching explicit interpreter-basename grant. Review that generated workflow before running it. The authored [workflow.yaml](workflow.yaml) remains readable and editable source.

On Windows, Python socket startup also requires `SYSTEMROOT`. Setup adds an explicit `SYSTEMROOT` environment reference only to the local `service-probe` actions and a matching `environmentAllowlist` grant. It records the variable name, never its value, and fails if the prerequisite is absent or conflicts with an authored environment setting or explicit allowlist. Review this declared prerequisite in `local.*.yaml`; other host environment variables are not inherited. Keep `SYSTEMROOT` available when running or resuming the workflow.

The complete package contains:

```text
17-compensated-rollout/
  README.md
  example.json
  fixtures/desired-service.json
  fixtures/service.json
  format_operations.py
  helper.py
  local_service.py
  operations.py
  reconcile.workflow.yaml
  requirements.txt
  service_operations.py
  setup.py
  workflow.yaml
  yaml_io.py
```

Setup creates local configuration and outputs separately. Keep `state.db` and `artifacts/` when investigating a run.

## Start the disposable service

After setup, start the package's loopback service in a separate terminal:

```sh
.venv/bin/python local_service.py
```

It prints a loopback URL and records `service-endpoint.json`. Keep it running while the workflow probes the service. Stop this exact process with Ctrl+C when finished. The server serves this package's `artifacts` directory; do not place secrets there.

## Run and inspect

```sh
agentctl check local.workflow.yaml --workspace .
agentctl plan local.workflow.yaml --workspace .
agentctl run local.workflow.yaml --workspace . --db state.db --output json --color never
```

Copy `runId` from the JSON result, then inspect it:

```sh
agentctl inspect RUN_ID --db state.db --output json --color never
```

## Follow the YAML

The workflow captures the real prior local state before mutation. On failure, inspect the applied effect, preview compensation, then execute the declared inverse and probe the restored state. Compensation is a separate auditable operation.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/17-compensated-rollout/workflow.yaml language=yaml -->

## Preview and execute compensation

After the intentional health failure, copy the failed source `runId`. Inspect it before executing an inverse:

```sh
agentctl compensate SOURCE_RUN_ID --plan --db state.db --workspace .
agentctl compensate SOURCE_RUN_ID --db state.db --workspace . --output json
agentctl inspect COMPENSATION_RUN_ID --db state.db --output json
```

Review the exact inverse task and captured prior state in the plan. A compensation command can pause for its own approvals; inspect each request rather than assuming forward approval authorizes every inverse.

## Verify the restored service

Keep the loopback service running. Execute the separate reconciliation workflow against the captured before-state:

```sh
agentctl check local.reconcile.workflow.yaml --workspace .
agentctl plan local.reconcile.workflow.yaml --workspace .
agentctl run local.reconcile.workflow.yaml --workspace . --db state.db --output json
agentctl inspect RECONCILIATION_RUN_ID --db state.db --output json
```

The result must contain `verified: true` and HTTP status `200` in `artifacts/reconciliation.json`. The helper compares the actual HTTP response with `artifacts/prior-service.json`; a successful inverse command alone does not establish that the running service recovered.

## Expected result

The restored values must match the captured prior state, not a hardcoded restored label. Inspect compensation lineage and the reconciliation report. A fixed failing health fixture remains useful for testing the failure path.

Selected fields from the recorded local walkthrough, after restoring a deliberately changed prior version and running the separate reconciliation workflow:

```json
{
  "verified": true,
  "httpStatus": 200,
  "healthy": true,
  "version": "7.8.9",
  "scope": "real HTTP probe of disposable loopback state"
}
```

## Use your own data

Before the first `setup.py` invocation in a new package, edit `fixtures/service.json`, including a different version or additional nested values. Setup copies that initial file only when `artifacts/service.json` is absent. Run the failure, compensation and reconciliation commands above and verify that the inverse restores those exact prior bytes too. Keep the before-state snapshot until compensation and reconciliation are complete. Review inverse authority as carefully as forward mutation authority.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Compensation can fail or become uncertain independently. Never describe it as transactional rollback. Stop and investigate a restoration mismatch; do not overwrite the expected state merely to satisfy a fixture assertion.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
