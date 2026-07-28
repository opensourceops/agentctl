# Integrate agentctl into CI/CD

`agentctl` is one OCI step inside the pipeline. The CI system checks out code, injects secrets, schedules work, retains state, and collects artifacts. `agentctl` validates and executes one bounded workflow with durable local history.

## Shared container contract

Every platform uses the same paths:

| Mount | Access | Purpose |
| --- | --- | --- |
| `/config` | read-only | reviewed workflow, inputs, and packs |
| `/workspace` | normally read-only | checked-out source and fixtures |
| `/state` | writable and retained | SQLite database, resume, replay, repair, approvals |
| `/artifacts` | writable and collected | declared reports and outputs |

Use `--output json --color never`. A successful workflow exits `0`. Validation exits `2`; policy or a pending approval exits `3`; run failure exits `4`; persistence exits `5`; provider or protocol failure exits `6`; cancellation exits `130`.

## Generic step

From a CI workspace that contains `config/workflow.yaml`:

```text
mkdir -p .agentctl-state artifacts
docker run --rm --read-only --user 65532:65532 \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --mount type=bind,src="$PWD/config",dst=/config,readonly \
  --mount type=bind,src="$PWD",dst=/workspace,readonly \
  --mount type=bind,src="$PWD/.agentctl-state",dst=/state \
  --mount type=bind,src="$PWD/artifacts",dst=/artifacts \
  ghcr.io/OWNER/agentctl:0.3.0 \
  run /config/workflow.yaml --workspace /workspace \
  --db /state/runtime.db --output json --color never
```

Replace the image owner and tag with a reviewed image. The repository does not claim that this example image exists publicly.

## Platform guides

The [container contract](../CONTAINER.md) contains complete examples for GitHub Actions, GitLab CI, Jenkins, Harness CI, Kubernetes Job, and Kubernetes CronJob. Each example preserves the same entrypoint, mounts, outputs, secrets, and exit behavior.

Their current evidence level is documentation or syntax review unless stated otherwise. The local container acceptance validates the generic contract, not every hosted platform.

## Inputs and structured output

Mount an ordinary JSON file under `/config` and pass `--inputs-file
/config/inputs.json`, or use repeated non-secret `--input KEY=VALUE`. Provider
credentials must be typed environment or mounted-file references. Capture
stdout as one `agentctl.dev/cli/v1` JSON envelope and archive declared files
from `/artifacts`.

## Approvals in pipelines

A non-interactive approval does not wait for stdin. It persists a request, exits `3`, and requires the same `/state` data in a later operator-controlled job. That job lists and resolves the approval, then calls `resume`. If your pipeline cannot retain protected state between jobs, configure policy to deny or fail instead of using approvals.

## Selective repair in pipelines

Keep the failed terminal `/state` and durable workspace, publish a reviewed corrected workflow, and run an effect-free planning step first:

```text
agentctl repair /config/repaired.yaml SOURCE_RUN_ID --from failed_task --plan \
  --workspace /workspace --db /state/runtime.db --output json --color never
```

Permit the execution step only when the plan exits `0` and the machine output's source run, target digest, roots, fresh effects, and approvals match the review. Exit `3` can also mean a blocked repair plan, so distinguish `kind: RepairPlan` from a pending run approval. Retain the new repair run ID as independent audit evidence.

## Retention and recovery

Collect `/state` even on failure when recovery or audit matters. It can contain confidential prompts and outputs, so apply protected artifact access and a short, documented retention period. Keep `/artifacts` according to the report's classification.

Do not set a pipeline retry policy that blindly repeats exit `5`, `6`, or `130`. Inspect the run first, because an external effect may be uncertain.

## Security checklist

- Pin the workflow and image version.
- Run as non-root with a read-only root filesystem.
- Drop capabilities and deny unneeded egress.
- Mount the workspace read-only unless a reviewed write is required.
- Inject secrets by environment reference or read-only mounted file and never echo them.
- Treat remote content and model output as untrusted.
- Retain state for approval or recovery, then delete it under policy.
- Set the external platform's overlap and timeout controls.
