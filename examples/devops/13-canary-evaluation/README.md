# 13. Evaluate a canary

**For:** SRE. **Level and evidence:** Beginner; offline local decision, no monitoring-service access.

Route a candidate to promote, rollback or hold using supplied request/error counts and minimum evidence thresholds.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 13 --output ./example-13
```

Enter the extracted directory containing `setup.py`. You need Python 3.11 or newer. Create an isolated environment and install the pinned example dependencies:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python setup.py
```

On Windows, use `.venv\Scripts\python.exe` in place of `.venv/bin/python`. Setup records the selected interpreters and prepares `local.workflow.yaml` with a matching explicit interpreter-basename grant. Review that generated workflow before running it. The authored [workflow.yaml](workflow.yaml) remains readable and editable source.

The complete package contains:

```text
13-canary-evaluation/
  README.md
  example.json
  fixtures/metrics.json
  format_operations.py
  helper.py
  local_service.py
  operations.py
  requirements.txt
  service_operations.py
  setup.py
  workflow.yaml
  yaml_io.py
```

Setup creates local configuration and outputs separately. Keep `state.db` and `artifacts/` when investigating a run.

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

The workflow validates integer counts, aggregates requests and errors, checks minimum sample and request counts, then uses a typed router. Insufficient evidence selects hold; it must not promote merely because there were zero observed errors.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/13-canary-evaluation/workflow.yaml language=yaml -->

## Expected result

The report exposes totals, ratio, thresholds, sample count, `sufficientData` and route. Healthy data selects promotion, unhealthy data selects rollback, and insufficient data holds. In a fresh package, only the selected local branch artifact should exist. Use a fresh package for each branch demonstration; artifacts from older invocations are not evidence of the current decision.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "requests": 1000,
  "errors": 3,
  "errorRatio": 0.003,
  "sufficientData": true,
  "route": "promote"
}
```

## Use your own data

Use `--input metricsPath=fixtures/my-metrics.json --input errorLimit=0.01 --input minRequests=100 --input minSamples=2`. Set thresholds from your service objective and sampling window. These counts are supplied observations, not a live monitoring query.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Negative counts, errors greater than requests, boolean counts, out-of-range ratios or nonpositive evidence thresholds are invalid. Test both a high-error sample and a small sample before connecting this decision to any real deployment.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
