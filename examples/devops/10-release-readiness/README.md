# 10. Evaluate and enforce release readiness

**For:** Release engineer. **Level and evidence:** Intermediate; offline, no model.

Combine actual gate results with a package checksum and record a reproducible go/no-go decision.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 10 --output ./example-10
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
10-release-readiness/
  README.md
  example.json
  fixtures/gates.json
  fixtures/package.txt
  format_operations.py
  gate.workflow.yaml
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

Analysis requires a nonempty list of uniquely named checks with boolean passed values, plus a lowercase SHA-256 digest. It hashes the supplied package and adds its own digest check. Report generation and permission to release are separate boundaries.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/10-release-readiness/workflow.yaml language=yaml -->

## Expected result

The supplied failing gate produces a successful report-only run with `decision: no-go` and a `blocking` list. The explicit CI gate mode must exit nonzero before its downstream release action when any check or checksum fails.

## Enforce the decision in CI

Setup also prepares `local.gate.workflow.yaml`. Inspect its `report` → gate assertion → downstream marker dependency before using it:

```sh
agentctl check local.gate.workflow.yaml --workspace .
agentctl plan local.gate.workflow.yaml --workspace .
agentctl run local.gate.workflow.yaml --workspace . --db gate.db --output json
```

For the supplied no-go fixture in a fresh package, expect a nonzero exit and no downstream release marker. If reusing a workspace, do not mistake an older run's marker for permission from the current gate. The report remains inspectable. With valid passing input data, the assertion permits the marker. Keep this local marker as a demonstration until you replace it with a separately reviewed release action and explicit authority.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "decision": "no-go",
  "blocking": [
    "security-review"
  ],
  "mode": "analysis-only"
}
```

## Use your own data

Copy your gate result JSON and package inside the workspace, then use `--input gatesPath=fixtures/my-gates.json --input packagePath=fixtures/my-package.tar`. Populate results from actual commands and retain their source evidence; do not use fabricated booleans as release proof.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Missing checks, duplicate names, non-boolean values and malformed digests are rejected. A changed package creates a no-go. A go means only that the supplied gates and checksum passed; it cannot attest to omitted gates or the trustworthiness of their producer.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
