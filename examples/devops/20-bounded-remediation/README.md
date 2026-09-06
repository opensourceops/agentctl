# 20. Run a bounded remediation loop

**For:** SRE. **Level and evidence:** Intermediate for the deterministic offline path; advanced for the separate fake-agent or paid OpenAI proposal variants.

Repair an input-derived local configuration defect within a fixed loop and resource ceiling, then validate the actual artifact.

## Get the complete example

Install the [matching candidate binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 20 --output ./example-20
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
20-bounded-remediation/
  README.md
  contract.workflow.yaml
  example.json
  fixtures/configuration.json
  format_operations.py
  helper.py
  instructions/remediator.md
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

The primary YAML declares `prepare` → `repair` → `analyze` → `report`. `repair` is a deterministic action loop with at most three iterations; it reads the previous artifact, performs one bounded timeout reduction and computes completion from the actual resulting configuration. `analyze` rereads the final artifact and checks the threshold, preserved fields and digest before reporting success.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/20-bounded-remediation/workflow.yaml language=yaml -->

## Optional bounded proposal stage

`contract.workflow.yaml` and `openai.workflow.yaml` add a separate proposal loop, also capped at three attempts. The agent stages the configured target as strict decimal bytes. `validate-target` checks those bytes, the proposal, the unchanged source and configured maximum before allowing the same deterministic repair loop described above.

A model's `done` value only ends its proposal attempts. It cannot establish that the configuration was repaired, bypass `validate-target`, or replace final artifact verification. These variants exercise the agent/tool contract; the primary input-driven repair works without a model.

## Expected result

Inspect the original configuration, proposed change, final artifact, iteration count and usage records. Successful termination requires the actual configuration to meet the declared rule. Invalid proposals and bounded nonconvergence remain failures.

Actual `remediation.json` from the recorded provider-free walkthrough:

```json
{
  "retries": 3,
  "security": {
    "allowPrivilegeEscalation": false
  },
  "service": "worker",
  "timeoutSeconds": 30
}
```

## Use your own data

Pass `--input configurationPath=fixtures/my-configuration.json --input maxTimeout=30 --input maxReduction=120` for a new source. The supplied timeout of 300 follows 180 → 60 → 30. Changing the maximum to 45 produces 180 → 60 → 45. Each step reduces an excessive timeout by at most the selected reduction until it meets the maximum; already valid values remain unchanged. `artifacts/remediation.json` must preserve unrelated fields. Test several source values and thresholds, including a defect too large to converge within the loop ceiling. Keep tool, request, token, cost and iteration ceilings explicit. The primary path requires no provider. Use the opt-in live workflow only with a shared paid allowance.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Out-of-bounds or malformed proposals must leave no accepted repair artifact. For the supplied timeout of 300, `--input maxReduction=20` cannot reach 30 in three iterations and must fail. In an agent variant, repeated ineffective proposals must also exhaust its separate bounded attempt loop. Synthetic fake-provider prices exercise accounting; they are not a real bill.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
