# 07. Validate a vendored-code patch

**For:** CI developer. **Level and evidence:** Intermediate; offline, Python and Git.

Apply a supplied patch to vendored code in a disposable workspace and prove a behavior fails before the patch and passes afterward.

## Get the complete example

Install the [matching candidate binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 07 --output ./example-07
```

Enter the extracted directory containing `setup.py`. You need Python 3.11 or newer. Git is also required. Create an isolated environment and install the pinned example dependencies:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python setup.py
```

On Windows, use `.venv\Scripts\python.exe` in place of `.venv/bin/python`. Setup records the selected interpreters and prepares `local.workflow.yaml` with a matching explicit interpreter-basename grant. Review that generated workflow before running it. The authored [workflow.yaml](workflow.yaml) remains readable and editable source.

The complete package contains:

```text
07-dependency-update/
  README.md
  example.json
  fixtures/test_dependency.py
  fixtures/update.patch
  fixtures/vendor_version.py
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

The workflow keeps the original target, supplied change and test evidence inspectable. A clean patch application alone is insufficient: the behavioral test must demonstrate the defect and its correction.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/07-dependency-update/workflow.yaml language=yaml -->

## Expected result

Inspect `artifacts/proposed.patch`, the patched target and test results. This is a vendored-code fix. It does not resolve a package-manager dependency graph, download packages, or prove compatibility with every downstream consumer.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "baselineExit": 1,
  "updatedExit": 0,
  "tests": 3
}
```

## Use your own data

Supply `fixtures/update.patch`, the target source and focused test fixture. The workflow inputs `targetPath`, `patchPath` and `testPath` select these files; it does not infer a change by comparing against an updated.py file. Keep the target beneath the disposable workspace. Adapt the test to the behavior being corrected rather than changing assertions merely to match the proposed implementation.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Reject a patch that touches an unexpected path, does not apply, or leaves the test failing. Do not apply this tutorial directly to your production checkout; copy the validated change into a separately reviewed branch.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
