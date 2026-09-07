# 01. Diagnose a failed CI build

**For:** CI developer. **Level and evidence:** Beginner; deterministic offline parsing and decision validation. A separate OpenAI workflow is opt-in.

A build log contains an exception, but the failing line is buried in unrelated output. Parse the log and select an evidence-supported next action. The optional live variant delegates that bounded choice to an agent.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 01 --output ./example-01
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
01-ci-diagnosis/
  README.md
  contract.workflow.yaml
  example.json
  fixtures/build.log
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

The parser distinguishes missing imports, test assertions, malformed configuration, and unknown or mixed evidence. The decision names an allowed action and the exact lines supporting it. A deterministic verifier creates the recommendation from that action; arbitrary advisory prose is retained with `validated: false`.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/01-ci-diagnosis/workflow.yaml language=yaml -->

## Expected result

The supplied log identifies `missing_dependency`, supports `install_declared_dependency`, and cites the line containing the import error. Read both the observed report and the validated action in `artifacts/report.json`; do not interpret advisory prose as a tested remediation.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "analysis": {
    "action": "install_declared_dependency",
    "evidence": [
      "fixtures/build.log:3"
    ],
    "recommendation": "Install the declared missing dependency, then rerun the failed build.",
    "rootCause": "missing_dependency"
  },
  "advisory": {
    "text": "",
    "validated": false
  }
}
```

## Use your own data

Put your UTF-8 build log inside this package and set `--input logPath=fixtures/my-build.log`. Empty logs fail. A test assertion or malformed configuration changes the observed category; mixed causes remain unknown instead of forcing one root cause. `contract.workflow.yaml` retains a scripted fake-agent regression fixture. The primary workflow accepts changed inputs without a model; `openai.workflow.yaml` makes a separate bounded live request.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

A recommendation to ignore the dependency and mark the build successful is invalid, even if its citation is real. The action, observation and evidence relationship must all agree. This workflow proposes no patch and grants no release permission.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
