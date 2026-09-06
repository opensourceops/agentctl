# 03. Review a GitHub Actions pipeline

**For:** CI developer. **Level and evidence:** Intermediate; offline, Python and Git plus the declared validator.

Find excessive job permissions and missing execution bounds, inspect a narrow patch, and validate the resulting workflow.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 03 --output ./example-03
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
03-pipeline-review/
  README.md
  example.json
  fixtures/pipeline.yaml
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

The workflow separates local input review from the proposed change and validation. Review the permission rules and the patch before adapting them to a real repository. The local patch workspace preserves unrelated YAML fields.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/03-pipeline-review/workflow.yaml language=yaml -->

## Expected result

Inspect `artifacts/proposed.patch`, the patched workflow under `artifacts/patch-workspace`, and the structured report. The patch must apply cleanly and the resulting GitHub Actions document must preserve unrelated fields and meet the local permission/timeout rules. The separate actionlint path validates GitHub Actions syntax and expressions; a report with actionlint unverified does not establish that broader result. This does not execute the pipeline on GitHub.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "violations": [
    {
      "location": "/permissions",
      "rule": "least-privilege"
    },
    {
      "location": "/jobs/test/timeout-minutes",
      "rule": "bounded-job"
    }
  ],
  "validated": true,
  "actionlint": {
    "scope": "local permission/timeout rules only; GitHub Actions syntax and expressions require actionlint",
    "status": "unverified",
    "version": null
  }
}
```

## Use your own data

Replace `fixtures/pipeline.yaml` with your actual GitHub Actions YAML, or pass `--input sourcePath=fixtures/my-pipeline.yaml`. Review the explicit `contents: read` permission recipe and `maxTimeoutMinutes` input; reusable-workflow call jobs require a different recipe. Keep credentials and repository secrets out of copied input files. A job that legitimately publishes needs a separately reviewed permission decision; a generic rule must not silently grant it.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Add GitHub Actions syntax validation

For the optional broader check, download the archive for your operating system and architecture from the [actionlint 1.7.7 release](https://github.com/rhysd/actionlint/releases/tag/v1.7.7), verify it against that release's checksum file, and extract the executable into this package as `tools/actionlint` (`tools/actionlint.exe` on Windows). The workflow does not download tools. Keep this executable inside the reviewed workspace.

On macOS or Linux, verify the selected binary and require it explicitly:

```sh
tools/actionlint --version
agentctl run local.workflow.yaml --workspace . --db state.db --input requireActionlint=true --input actionlintPath=tools/actionlint --output json
agentctl inspect RUN_ID --db state.db --output json
```

On Windows, use `tools/actionlint.exe` for both the version command and `actionlintPath`. The helper requires version `1.7.7`. Inspect `artifacts/actionlint.txt` and the report's `actionlint.status`. This mode checks GitHub Actions syntax and expressions; optional ShellCheck and pyflakes discovery are disabled so the result does not depend on undeclared host tools. It still does not run hosted jobs.

## Failure and recovery

Malformed YAML, unsupported workflow structure or an invalid proposed patch stops the workflow. When `requireActionlint` is true, an unavailable or differently versioned actionlint also stops validation. A passing local validator does not establish that third-party Actions are trusted or that hosted jobs will pass.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
