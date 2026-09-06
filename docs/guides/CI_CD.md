# Integrate agentctl into CI/CD

The CI system checks out source, schedules work, selects a reviewed image, injects secrets and retains results. agentctl validates and executes one bounded workflow with durable local history. A model can propose a change only through its declared tools; the outer pipeline remains responsible for build, scan and publication authority.

## Choose the execution image

Use the minimal image when the pipeline invokes the agentctl entrypoint directly. It has no shell, Python, Git or Docker CLI. Use the tooling image for reviewed Python/Git process adapters or a CI Run step that executes a shell script. For local shell access, the tooling image needs the explicit `--entrypoint /bin/sh` override; its default entrypoint is still agentctl.

Set `AGENTCTL_IMAGE` to the reviewed digest for the chosen flavor from the [release summary](https://github.com/opensourceops/agentctl/releases). Mutable `latest` and `ci` tags are exploration conveniences after publication. A source-built image can validate unreleased work without inventing a published tag. See the [container walkthrough](../CONTAINER.md#run-your-first-image-workflow).

## Shared container contract

| Mount | Access | Purpose |
| --- | --- | --- |
| `/workspace/config` | Read-only | Reviewed workflow, inputs, variable files, instruction files and packs. |
| `/workspace` | Normally read-only | Checked-out source and fixtures; only an authorized patch area is writable. |
| `/state` | Writable and retained | SQLite history and sibling CAS for resume, replay, repair and approvals. |
| `/artifacts` | Writable and collected | Declared reports, with an explicit workflow writable-root grant. |
| `/run/secrets` | Read-only | Credential files under the workflow's explicit secret-file root. |

External instruction and variable files resolve from their workflow or pack origin and must remain inside the workspace's canonical read boundary. A separate `/config` mount outside `/workspace` does not grant ordinary file-read authority. Inputs, variables and secret references remain separate namespaces.

Provision writable directories for the selected non-root identity. On a Linux hosted runner, matching the non-root host UID/GID preserves narrow directory permissions and permits artifact collection. Keep the root filesystem read-only, drop capabilities, and deny unused egress. The image's default UID/GID remains 65532.

## Platform guides

The [container contract](../CONTAINER.md#pipeline-examples) provides complete GitHub Actions and Harness Run-step examples, plus integration requirements for GitLab, Jenkins and Kubernetes. The GitHub example invokes Docker from the hosted runner and keeps the Docker socket outside the workflow container. The Harness example selects a tooling image with a shell and executes agentctl directly without nested Docker.

Harness, GitLab, Jenkins and Kubernetes configuration guidance is not evidence of hosted execution. Check the exact source, image, platform and retained run evidence for the integration you use.

## Inputs and structured output

Mount ordinary JSON input under `/workspace/config` and pass `--inputs-file /workspace/config/inputs.json`, or use repeated non-secret `--input KEY=VALUE`. Ordered variable files use `--vars-file`; explicit variable overrides use `--var KEY=JSON`. Use `explain` to inspect winning origins without printing the values. Provider credentials use dedicated environment or mounted-file references.

Use `--output json --color never`. Success emits an `agentctl.dev/cli/v1` result on stdout; errors emit a versioned error on stderr. Capture both streams separately when a failed job needs diagnostics. A successful workflow exits `0`; validation exits `2`; policy or pending approval exits `3`; run failure exits `4`; persistence exits `5`; provider/protocol failure exits `6`; cancellation exits `130`. Inspect the envelope kind and state rather than interpreting every nonzero result as safely retryable.

## Approvals in pipelines

A non-interactive approval persists a request and exits `3`. An operator-controlled job must restore the same protected state, list and resolve the request using an authorized identity, then resume. If the pipeline cannot retain protected state between jobs, it cannot support this durable approval journey.

## Selective repair in pipelines

Keep the terminal state and required workspace, publish a reviewed corrected workflow, and plan without dispatching fresh work:

```sh
agentctl repair /workspace/config/repaired.yaml SOURCE_RUN_ID --from failed_task --plan \
  --workspace /workspace --db /state/runtime.db --output json --color never
```

Permit execution only when the plan's source run, target digest, roots, fresh effects and approvals match the review. Exit `3` can also mean a blocked repair plan; distinguish `kind: RepairPlan` from a pending run approval. Retain the new repair run ID as independent audit evidence. See [Selective repair](repair-a-failed-workflow.md).

## Retention and replay

Collect state even on failure when recovery or audit matters. It can contain confidential prompts, outputs and artifacts, so apply protected access and a documented retention period. Retain the database and sibling CAS together. Keep exported reports according to their classification.

Replay of the recorded agentctl run needs no fresh provider request and must run without provider credentials or network access. An encrypted database still needs its state-encryption key. Replay does not redo outer pipeline Docker builds, vulnerability scans or GitHub publication. Do not blindly retry exit `5`, `6` or `130`: inspect and reconcile any uncertain remote effect first.

## A complete remediation journey

The [container remediation example](../../examples/devops/21-container-remediation/README.md) connects real build and Trivy scan evidence to bounded analysis and patch agents, deterministic validation and a separately authorized draft-PR publisher. Its standalone package, prerequisites, workflow authority and exact validation evidence belong to that tutorial. Existing [DevOps examples](../../examples/devops/README.md) remain useful for credential-free policy, artifact and recovery practice.
