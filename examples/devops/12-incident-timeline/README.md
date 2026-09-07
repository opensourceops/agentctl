# 12. Build an incident timeline

**For:** SRE. **Level and evidence:** Beginner; deterministic offline parsing and decision validation. A separate OpenAI workflow is opt-in.

Normalize timestamped log events, cite their original lines and select an evidence-supported investigation action.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 12 --output ./example-12
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
12-incident-timeline/
  README.md
  contract.workflow.yaml
  example.json
  fixtures/incident.log
  format_operations.py
  helper.py
  instructions/analyst.md
  local_service.py
  openai.workflow.yaml
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

Every nonempty line must contain a timezone-bearing timestamp, service and message. The parser normalizes instants to UTC, sorts them and applies an explicit optional window. Duration is the observed interval, not automatically the outage duration or proof of causation.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/12-incident-timeline/workflow.yaml language=yaml -->

## Expected result

The report contains the ordered timeline, original line citations, `durationSeconds` and supported actions. The selected action must preserve the deterministic duration and cite its supporting evidence. Free-text advisory material is explicitly unvalidated.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "analysis": {
    "action": "inspect_connection_pool",
    "durationSeconds": 120.0,
    "evidence": [
      "fixtures/incident.log:2"
    ],
    "recommendation": "Inspect connection-pool capacity and saturation using the cited incident evidence."
  },
  "advisory": {
    "text": "",
    "validated": false
  }
}
```

## Use your own data

Use `--input logPath=fixtures/my-incident.log`. Optional `windowStart` and `windowEnd` inputs require timezones. Compare equivalent offset timestamps and out-of-order lines; event order should follow the instant rather than textual timestamp sorting. The primary workflow accepts changed logs without a model. `contract.workflow.yaml` retains a scripted fake-agent regression fixture, and `openai.workflow.yaml` supplies separately budgeted live analysis.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Naive timestamps, malformed lines, reversed windows and windows with no events fail. Advice unsupported by the cited incident pattern is rejected. A pool-capacity investigation is justified only by supporting pool evidence, not any arbitrary error log.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
