# 16. Retry and selectively repair a workflow

**For:** SRE. **Level and evidence:** Intermediate; offline recovery contract demonstration.

Recover a terminal test failure while reusing a successful upstream build boundary.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 16 --output ./example-16
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
16-retry-repair/
  README.md
  example.json
  fixtures/desired-service.json
  format_operations.py
  helper.py
  local_service.py
  operations.py
  repaired.workflow.yaml
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

Resume continues an interrupted nonterminal run. Retry creates a child attempt for failed boundaries of a compatible terminal workflow. Repair uses corrected workflow content and reuses only compatible successful boundaries. Preview reuse before dispatching either terminal recovery command.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/16-retry-repair/workflow.yaml language=yaml -->

## Retry or repair the terminal failure

The first run intentionally fails after the build. Copy its source `runId` and inspect it. Restore the disposable test dependency, then preview and run a retry:

```sh
.venv/bin/python -c "from pathlib import Path; Path('fixtures/retry-ready.txt').write_text('available\n')"
agentctl retry local.workflow.yaml SOURCE_RUN_ID --failed --plan --db state.db --workspace .
agentctl retry local.workflow.yaml SOURCE_RUN_ID --failed --db state.db --workspace . --output json
```

For a corrected suffix, use the prepared repaired workflow against the original source:

```sh
agentctl repair local.repaired.workflow.yaml SOURCE_RUN_ID --from test --plan --db state.db --workspace .
agentctl repair local.repaired.workflow.yaml SOURCE_RUN_ID --from test --db state.db --workspace . --output json
agentctl inspect CHILD_RUN_ID --db state.db --output json
```

The repair path demonstrates a reviewed workflow change; it does not overwrite the failed source. Inspect both child attempts and confirm that the build is reused.

## Expected result

The initial test fails after the build succeeds. Inspect the source run, then the retry or repair child and its lineage. The build counter must remain unchanged when that successful boundary is reused.

Selected fields from the recorded local walkthrough, after selective repair reused the confirmed build:

```json
{
  "verified": true,
  "healthy": true,
  "mutation": 1,
  "dependencyChecked": false,
  "scope": "actual local configuration validation; this check does not claim HTTP or external deployment health"
}
```

## Use your own data

Restore the failing fixture input for an unchanged-workflow retry, or use the supplied repaired workflow to change the failing suffix. Copy each returned child run ID separately from the original source ID. The source history remains available.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Use `--plan` first. Changed inputs, instructions, policy or upstream task semantics can invalidate reuse. An uncertain effect is a reconciliation problem, not a reason to force retry or repair.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
