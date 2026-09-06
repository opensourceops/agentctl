# 06. Review a Terraform plan

**For:** Platform engineer. **Level and evidence:** Intermediate; offline plan analysis and a local approval demonstration. No Terraform apply.

Review Terraform or OpenTofu plan JSON, then require an explicit approval before writing an isolated demonstration result.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 06 --output ./example-06
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
06-terraform-plan/
  README.md
  example.json
  fixtures/plan.json
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

The run pauses before governed mutations. Use the returned run and approval identifiers:

```sh
agentctl approvals --db state.db list RUN_ID --output json
agentctl approvals --db state.db approve APPROVAL_ID --actor local-reviewer --reason "Reviewed the exact disposable operation"
agentctl resume RUN_ID --db state.db --output json
```

Review each pending request before approving it. A later artifact write may require a separate approval; list the pending requests again after each resume. Local database ownership is the authorization boundary for approval commands.

Copy `runId` from the JSON result, then inspect it:

```sh
agentctl inspect RUN_ID --db state.db --output json --color never
```

## Follow the YAML

The analysis consumes plan JSON and identifies governed changes. A separate task pauses before the local mutation. An approval binds to the pending operation reviewed in the plan; it is not blanket permission to apply infrastructure.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/06-terraform-plan/workflow.yaml language=yaml -->

## Expected result

The input is a saved JSON plan such as output from `terraform show -json` or `tofu show -json`. The report and local mutation artifact remain inside the package. No cloud provider, state backend or production resource is contacted.

Selected fields from the supplied fixture's `artifacts/report.json`:

```json
{
  "allowed": true,
  "changes": [
    {
      "actions": [
        "create"
      ],
      "address": "local_file.fixture"
    }
  ],
  "denied": [],
  "targetPath": "artifacts/local-state.json"
}
```

## Use your own data

Copy the plan JSON inside this package. Plans can contain sensitive values: redact or protect them and the resulting SQLite history. Adjust the explicit rules for your own resource types before reviewing a real plan.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

Denied changes must leave the application mutation absent. The direct run can pause with an approval request; inspect its task, target and digest before approving. This example never invokes `terraform apply` or `tofu apply`.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
