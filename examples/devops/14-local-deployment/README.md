# 14. Approve a disposable local deployment

**For:** Platform engineer. **Level and evidence:** Intermediate; local contract demonstration, Python and a loopback HTTP service.

Review an exact local state change, approve it and inspect an observable health result from a disposable service.

## Get the complete example

Install the [matching agentctl binary](../../../docs/guides/INSTALLATION.md). Download this tutorial's complete package from the documentation site and extract it into an empty directory. When working from the source checkout, create the same package with:

```sh
python3 examples/devops/package.py --example 14 --output ./example-14
```

Enter the extracted directory containing `setup.py`. You need Python 3.11 or newer. Create an isolated environment and install the pinned example dependencies:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python setup.py
```

On Windows, use `.venv\Scripts\python.exe` in place of `.venv/bin/python`. Setup records the selected interpreters and prepares `local.workflow.yaml` with a matching explicit interpreter-basename grant. Review that generated workflow before running it. The authored [workflow.yaml](workflow.yaml) remains readable and editable source.

On Windows, Python socket startup also requires `SYSTEMROOT`. Setup adds an explicit `SYSTEMROOT` environment reference only to the local `service-probe` actions and a matching `environmentAllowlist` grant. It records the variable name, never its value, and fails if the prerequisite is absent or conflicts with an authored environment setting or explicit allowlist. Review this declared prerequisite in `local.*.yaml`; other host environment variables are not inherited. Keep `SYSTEMROOT` available when running or resuming the workflow.

The complete package contains:

```text
14-local-deployment/
  README.md
  example.json
  fixtures/desired-service.json
  fixtures/service.json
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

## Start the disposable service

After setup, start the package's loopback service in a separate terminal:

```sh
.venv/bin/python local_service.py
```

It prints a loopback URL and records `service-endpoint.json`. Keep it running while the workflow probes the service. Stop this exact process with Ctrl+C when finished. The server serves this package's `artifacts` directory; do not place secrets there.

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

The normal workflow makes deployment and health validation visible. Setup starts the explicitly local service separately; it is not production hosting. Inspect the initial state and intended version before the mutation approval, then compare the post-change probe.

[Open the complete workflow](workflow.yaml) to inspect its inputs, task dependencies, grants and bounds. The site embeds the same source below; editing a helper does not replace review of its host-process authority.

<!-- agentctl-include: examples/devops/14-local-deployment/workflow.yaml language=yaml -->

## Expected result

Retain the before and after probe records, service document, approval ID and final report. A static document served over loopback demonstrates file-backed deployment mechanics. It does not prove an application rollout, readiness probe in Kubernetes or zero-downtime upgrade.

Selected fields from the recorded local walkthrough, after reviewed approvals and an actual loopback HTTP probe:

```json
{
  "verified": true,
  "httpStatus": 200,
  "healthy": true,
  "version": "2.0.0",
  "scope": "real HTTP probe of disposable loopback state"
}
```

## Use your own data

Edit the desired local service data and use a fresh disposable package. Review the helper and loopback address before running it. Keep service setup and cleanup explicit, and stop the service belonging to this example when finished.

Paths in these inputs stay inside the package's reviewed workspace. Use ordinary vars for non-secret configuration only. An input or variable does not grant authority to a new filesystem path, command or network destination.

## Failure and recovery

A rejected approval must leave the prior state intact. A failed health check must prevent a success report. Do not approve a pending operation merely because its task name sounds harmless; inspect its reviewed destination and content.

For a terminal successful run, reconstruct the recorded result without fresh effects:

```sh
agentctl replay RUN_ID --db state.db --output json --color never
```

For a failure, preserve the database and inspect task/effect status before choosing [resume, retry or repair](../../../docs/DURABLE_EXECUTION.md). A new run is a fresh invocation, not recovery of the old one.

## Authority and cleanup

The command uses the selected virtual environment's absolute interpreter path, while `processAllowlist` authorizes its basename. That generic Python grant trusts the reviewed helper; it does not pin one script or independently constrain its child processes. It is not an operating-system sandbox for every file access or child process made by Python. Only run the complete reviewed package on a trusted local machine or disposable runner. No production system is modified by this tutorial.

After saving needed reports and stopping this example's local service if present, remove only its disposable directory. The [optional acceptance suite](../README.md#contributor-verification) exercises additional denials, replay and failure injection; it is not required to run the published workflow.
