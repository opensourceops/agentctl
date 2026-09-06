# 09. Generate evidenced release notes

**For:** Release engineer. **Level and evidence:** Beginner; offline, Python and Git.

Generate a changelog from an existing Git repository and explicit commit range without mutating that repository.

## Get the complete example

Install the [matching candidate binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 09 --output ./example-09
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
09-release-notes/
  README.md
  example.json
  fixtures/history.json
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

Setup may create the disposable fixture history once. The operational action only resolves the supplied refs, reads commits and changed paths, and writes release notes beneath `artifacts`. The range excludes `fromRef` and includes `toRef`, with a 500-commit ceiling.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/09-release-notes/workflow.yaml language=yaml -->

## Expected result

Inspect `artifacts/CHANGELOG.md` and the report containing resolved full commit IDs. Each note names its source commit and changed files. Two identical runs produce the same changelog bytes and leave repository history unchanged.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "commitsVerified": 3,
  "outputPath": "artifacts/CHANGELOG.md"
}
```

The complete report also contains full commit IDs and changed-file evidence. Your repository or freshly prepared fixture history can have different commit IDs.

## Use your own data

Copy your repository beneath the package, then pass `--input repositoryPath=fixtures/my-repo --input fromRef=v0.2.0 --input toRef=v0.3.0`. Use explicit reviewed refs; refs beginning with an option prefix are rejected. The helper does not fetch remotes.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Missing refs, an excessive range or an output path outside `artifacts` fail. Repeating analysis is idempotent. Fixture preparation refuses to alter an existing unmarked user repository; use a new directory when changing the fixture history.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
