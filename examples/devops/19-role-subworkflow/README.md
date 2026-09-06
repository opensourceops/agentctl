# 19. Review a proposed change with bounded roles

**For:** Platform engineer. **Level and evidence:** Advanced; deterministic offline change review with typed handoffs. Separate fake-agent and paid OpenAI variants.

Connect planner, reviewer and executor through typed handoffs for a substantive, bounded local change.

## Get the complete example

Install the [matching candidate binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 19 --output ./example-19
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
19-role-subworkflow/
  README.md
  contract.workflow.yaml
  example.json
  fixtures/change.txt
  fixtures/configuration.json
  format_operations.py
  helper.py
  instructions/executor.md
  instructions/planner.md
  instructions/reviewer.md
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

The primary workflow prepares a source digest and requested change, then calls a reusable sub-workflow with explicit plan, review, validation and handoff tasks. A failed review assertion blocks the executor. The final action accepts only the matching reviewed timeout and source digest, writes an output copy and verifies it. The separate agent variants assign planner, reviewer and executor roles distinct tool visibility; they must pass the same deterministic review boundary.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/19-role-subworkflow/workflow.yaml language=yaml -->

## Expected result

Inspect the proposed values, reviewer decision and actual resulting artifact. Rejection must prevent executor mutation. The declared sub-workflow and individual agent tool lists make role boundaries visible in YAML.

Actual `reviewed-configuration.json` from the recorded provider-free walkthrough:

```json
{
  "retries": 2,
  "security": {
    "runAsNonRoot": true
  },
  "service": "api",
  "timeoutSeconds": 30
}
```

## Use your own data

Copy a configuration into the package and pass `--input configurationPath=fixtures/my-configuration.json --input proposedTimeout=25 --input maxTimeout=30`. A proposal of 25 is within that bound; 31 must be rejected. The resulting `artifacts/reviewed-configuration.json` preserves unrelated fields and leaves the supplied source unchanged. The primary workflow adapts without a model. `contract.workflow.yaml` teaches the agent protocol with scripted responses; `openai.workflow.yaml` separately evaluates bounded model behavior.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

A malformed handoff, an out-of-bounds proposal or reviewer rejection must block the executor. Typed schema compliance alone does not prove semantic correctness; the deterministic artifact check is the final boundary.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
