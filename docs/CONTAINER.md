# Run agentctl in a container

Use the minimal image for workflows implemented by the Rust runtime. Use the tooling image when a reviewed process adapter needs Python or Git, or a CI Run step needs a shell. Both run as UID/GID 65532 by default and have `/usr/local/bin/agentctl` as their entrypoint.

## Choose an image

| Image | Included | Appropriate use |
| --- | --- | --- |
| Minimal | agentctl, runtime libraries, public CA roots | Built-in actions, native providers, durable inspection and replay. No shell, Python, Git, or Docker CLI. |
| Tooling | The CLI plus `/bin/sh`, `/usr/bin/python3`, Git, Python packaging and YAML modules, and public CA roots | Reviewed Python/Git adapters and shell-based CI Run steps. No Docker daemon/socket, GitHub publisher credential, or automatic process grant. |

Published images use the `docker.io/opensourceops/agentctl` repository. The mutable `latest` tag selects the minimal image; `ci` selects tooling. Use them for exploration only after the corresponding release is published. For CI, set `AGENTCTL_IMAGE` to the exact `docker.io/opensourceops/agentctl@sha256:…` reference recorded in the reviewed release summary, selecting the desired flavor. A tag name is not evidence that an image has been published or tested on your platform.

For changes that have not been published, build from the reviewed source checkout:

```sh
docker build --file Containerfile --tag agentctl:local .
docker build --file Containerfile --target tooling --tag agentctl-tooling:local .
```

For a local tooling check, override the entrypoint explicitly:

```sh
docker run --rm --read-only --network none --entrypoint /bin/sh \
  agentctl-tooling:local -c 'git --version && /usr/bin/python3 -c "import packaging, ruamel.yaml"'
```

Appending a shell command after the image name without `--entrypoint /bin/sh` passes it to agentctl instead. The minimal image has no shell for this invocation.

The default final build target is minimal. The [release process](RELEASE_PROCESS.md) records supported platforms, source relationships, checksums and image digests. A tooling image permits more executables to exist; the workflow must still explicitly authorize each process action. Allowing Python or a shell grants the capabilities of that interpreter to the reviewed script; it is not a file-level sandbox.

This whole-workflow container is separate from action-level `isolation: container`, where a host agentctl process invokes an exact local Docker/Podman image with restricted mounts, resources and networking. Neither image includes an engine, and the following walkthrough mounts no engine socket. See [Process isolation](guides/PROCESS_ISOLATION.md).

## Run your first image workflow

This example needs a Docker-compatible engine and the locally built minimal image above. It runs without credentials or network access. Start from a new directory as a non-root host user:

```sh
mkdir agentctl-container-first-run
cd agentctl-container-first-run
umask 077
mkdir -p config state artifacts
test "$(id -u)" -ne 0
export AGENTCTL_IMAGE=agentctl:local
```

Save this complete document as `config/workflow.yaml`:

```yaml
apiVersion: agentctl.dev/v1
kind: Workflow
metadata:
  name: container-greeting
spec:
  inputs:
    name: world
  varsFiles: [defaults.yaml]
  policy:
    workspaceRoot: .
    writableRoots: [/artifacts]
    approval: never
  actions:
    assign:
      kind: builtin.assign
    write:
      kind: builtin.write
  tasks:
    - id: greet
      uses: action:assign
      with:
        message: "${{ vars.salutation }}, ${{ inputs.name }}"
    - id: report
      uses: action:write
      needs: [greet]
      with:
        path: /artifacts/greeting.txt
        content: "${{ tasks.greet.output.output.message }}"
  outputs:
    greeting: "${{ tasks.greet.output.output.message }}"
    artifact: /artifacts/greeting.txt
```

Save `config/defaults.yaml` beside it:

```yaml
salutation: hello
```

Define this host-shell function. It preserves the image entrypoint, mounts reviewed files read-only and gives only state and reports writable mounts. The state directory is not also exposed through a workspace read mount:

```sh
agentctl_container() {
  test "$(id -u)" -ne 0 || return 2
  docker run --rm --read-only --network none \
    --user "$(id -u):$(id -g)" --cap-drop ALL \
    --security-opt no-new-privileges \
    --tmpfs /tmp:rw,noexec,nosuid,size=16m \
    --mount type=bind,src="$PWD/config",dst=/workspace/config,readonly \
    --mount type=bind,src="$PWD/state",dst=/state \
    --mount type=bind,src="$PWD/artifacts",dst=/artifacts \
    "$AGENTCTL_IMAGE" "$@"
}
agentctl_container version --output json --color never
agentctl_container check /workspace/config/workflow.yaml --workspace /workspace
agentctl_container plan /workspace/config/workflow.yaml --workspace /workspace
agentctl_container run /workspace/config/workflow.yaml --workspace /workspace \
  --db /state/runtime.db --output json --color never > artifacts/run.json
cat artifacts/greeting.txt
```

The run must exit `0`, return `state: succeeded` in its JSON result, and write `hello, world` to `artifacts/greeting.txt`. `state/runtime.db` and its sibling artifact store retain the execution. The workflow grants only the demonstrated report write; no model, host process or network operation is involved.

The explicit `--user` matches the non-root owner of the host directories, avoiding world-writable permissions and a post-run ownership repair. For a fixed container UID, provision equivalent narrowly writable directories for UID/GID 65532 before running. Rootless Podman has additional UID mapping semantics: use its documented host-user mapping for your installation and verify the mounted directory permissions before running. Merely finding an engine executable does not establish usable mounts.

## Inspect, replay and recover

Read `runId` from `artifacts/run.json`, substitute it for `RUN_ID`, and keep the same function and state mount:

```sh
agentctl_container inspect RUN_ID --db /state/runtime.db --output json --color never
agentctl_container replay RUN_ID --db /state/runtime.db --output json --color never
```

Replay reconstructs recorded output without new provider calls, file writes or external effects. The function enforces `--network none` and forwards no provider key. Keep the SQLite database and its sibling `artifacts` CAS together; encrypted state still requires its state-encryption key. An external Docker build, Trivy scan or GitHub publication performed by the surrounding pipeline is outside agentctl replay.

For a failed run, inspect before choosing a recovery command. `resume` continues captured state; terminal `retry` requires matching reviewed workflow inputs; `repair --plan` compares a corrected workflow with the source run without dispatching work:

```sh
agentctl_container repair /workspace/config/repaired.yaml SOURCE_RUN_ID \
  --from failed_task --plan --workspace /workspace --db /state/runtime.db \
  --output json --color never
```

The corrected file must exist before this planning command. Reused boundaries need no fresh provider credential. Executing fresh tasks may require a separately reviewed network and secret configuration. An uncertain non-idempotent mutation requires reconciliation; a pipeline retry must not turn it into a duplicate effect. See [Selective repair](guides/repair-a-failed-workflow.md).

To clean up this disposable walkthrough, remove its `agentctl-container-first-run` directory after retaining any report or history you need. Do not remove an operational database as a recovery technique.

## Mounts and inputs

| Container path | Access | Purpose |
| --- | --- | --- |
| `/workspace/config` | Read-only | Workflow, pack, instruction and ordinary variable files, together with their relative paths. |
| `/workspace` | Read-only by default | Reviewed source and input fixtures. Mount only a separately authorized patch area writable when a workflow needs it. |
| `/state` | Writable and retained | SQLite history and sibling CAS blobs needed for recovery. |
| `/artifacts` | Writable, explicitly listed in `policy.writableRoots` | Declared reports collected by the outer pipeline. Ordinary reads remain confined to the workspace. |
| `/run/secrets` | Read-only, explicitly listed in `policy.secretFileRoots` | Dedicated credential files, separate from configuration and artifacts. |

`instructionsFile` and workflow/agent/task `varsFiles` resolve relative to their declaring workflow or pack. They must remain inside the selected workspace's canonical read boundary. Mounting those files in a separate `/config` outside `--workspace /workspace` does not authorize reading them. Missing files, traversal and escaping symlinks fail before dispatch. See [Variables and instruction files](VARIABLES.md).

Use `--inputs-file /workspace/config/inputs.json` or repeated `--input KEY=VALUE` for typed `inputs`. Use ordered `--vars-file /workspace/config/overrides.yaml` and explicit `--var KEY=JSON` for `vars`. The namespaces stay separate. `explain` reports winning variable origins without printing their values. Instruction files are reviewed text, not credentials. Ordinary variables are retained configuration and must not contain keys or tokens.

A model-dependent workflow may receive `--env OPENAI_API_KEY` from a protected host environment or a read-only secret file. Forward only the name, never a literal value in the command. Remove `--network none` only for a reviewed network-enabled run, retain the workflow's destination policy, and apply the platform's egress controls. A credential does not grant network authority. [Secret references](guides/SECRET_REFERENCES.md) gives the mounted-file contract.

## Pipeline examples

The pipeline owns checkout, scheduling, image selection, secret injection and artifact retention. The agentctl image owns one bounded execution. Exit `3` can represent a denied operation or a durably pending approval; examine the JSON kind and run state. Preserve protected state before resolving an approval and resuming. Do not blindly repeat uncertain failures.

### GitHub Actions

This Linux hosted-runner example uses Docker on the runner and the image's agentctl entrypoint. Configure the repository variable `AGENTCTL_IMAGE` with a reviewed image digest. The checkout contains the complete `config/workflow.yaml` and `config/defaults.yaml` shown above; no OpenAI secret is needed.

```yaml
jobs:
  report:
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Run the reviewed workflow
        env:
          AGENTCTL_IMAGE: ${{ vars.AGENTCTL_IMAGE }}
        run: |
          set -eu
          test "$(id -u)" -ne 0
          case "$AGENTCTL_IMAGE" in
            docker.io/opensourceops/agentctl@sha256:*) ;;
            *) echo "Set AGENTCTL_IMAGE to the reviewed digest" >&2; exit 2 ;;
          esac
          umask 077
          mkdir -p .agentctl-state artifacts
          docker run --rm --read-only --network none \
            --user "$(id -u):$(id -g)" --cap-drop ALL \
            --security-opt no-new-privileges \
            --tmpfs /tmp:rw,noexec,nosuid,size=16m \
            --mount type=bind,src="$GITHUB_WORKSPACE/config",dst=/workspace/config,readonly \
            --mount type=bind,src="$GITHUB_WORKSPACE/.agentctl-state",dst=/state \
            --mount type=bind,src="$GITHUB_WORKSPACE/artifacts",dst=/artifacts \
            "$AGENTCTL_IMAGE" run /workspace/config/workflow.yaml \
            --workspace /workspace --db /state/runtime.db \
            --timeout-seconds 600 --output json --color never > artifacts/run.json
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        if: always()
        with:
          name: agentctl-state-and-reports
          include-hidden-files: true
          path: |
            .agentctl-state/
            artifacts/
          retention-days: 7
```

Use protected artifact access when inputs or outputs are confidential. The JSON result is an outer CI file; declared workflow artifacts are additionally represented in durable state. This simple job has no repository write token, engine socket inside the agentctl container, or model task. The [container remediation example](../examples/devops/21-container-remediation/README.md) separates real build/scan, bounded agent work and draft-PR publication into their respective trust boundaries.

### Harness CI Run step

A Run step executes its command through the selected shell. Select the tooling image by digest through the required `image` runtime input below; a minimal image has no `/bin/sh` for this script. Configure the stage's workspace permissions for non-root UID/GID 65532 and retain `.agentctl-state` and reports using your approved artifact step even if execution fails.

This example uses the generic workspace-relative workflow from [Getting started](guides/GETTING_STARTED.md), saved as `config/workflow.yaml`; its deterministic assignment requires no extra writable report mount or provider key.

```yaml
- step:
    type: Run
    name: Run agentctl
    identifier: run_agentctl
    spec:
      image: <+input>
      shell: Sh
      command: |-
        set -eu
        umask 077
        mkdir -p .agentctl-state artifacts
        agentctl check config/workflow.yaml --workspace "$PWD"
        agentctl run config/workflow.yaml --workspace "$PWD" \
          --db .agentctl-state/runtime.db --timeout-seconds 600 \
          --output json --color never > artifacts/run.json
```

No Docker socket is needed inside this step. For a provider workflow, inject the separately configured Harness secret as an environment reference only in the step that needs it and apply its approved egress policy. This guide's Harness coverage is vendor syntax review; actual execution depends on the selected Harness infrastructure, pull connector, shell and writable workspace. See [Harness Run step settings](https://developer.harness.io/3k-docs/continuous-integration/use-ci/run-step-settings/).

### GitLab CI

Use a runner whose Docker client can address the intended daemon and whose checkout paths are visible to that daemon. Apply the GitHub example's digest selection, non-root directory ownership, read-only input mounts and explicit writable mounts to that runner. A sibling Docker container's filesystem path is not automatically a host-daemon bind path. Set protected/masked provider variables only for model-dependent jobs, and retain state and reports with `artifacts: when: always` under your retention policy. No hosted GitLab run is implied by this configuration guidance.

### Jenkins declarative pipeline

Use a controlled non-root Docker-capable agent, the same digest-pinned invocation and host-owned writable directories. Wrap a provider-dependent step with the approved Jenkins credential binding and forward the environment name only. Archive state and reports in an `always` post condition. Do not give a model action the Jenkins credential store or Docker socket. A pipeline running on a different agent must restore both the database and its CAS before recovery. No hosted Jenkins run is implied by this guidance.

### Kubernetes Job or CronJob

Use the minimal image with an `args` array for direct agentctl execution, or the tooling image only when a reviewed wrapper needs its tools. Set the container image to a reviewed digest. Project workflow and external configuration together beneath `/workspace/config`, mount the source workspace read-only, and provision separate state/report volumes with permissions for UID/GID 65532. Set `runAsNonRoot`, `allowPrivilegeEscalation: false`, a read-only root, dropped capabilities and a bounded temporary volume. Project credential files separately under the authorized secret root.

Retain state on a suitable single-owner volume when recovery across Pods matters. SQLite ownership and locking still apply. A CronJob should normally set `concurrencyPolicy: Forbid`, a bounded deadline and a retry policy that respects uncertain effects; a one-time `batch/v1` Job uses the same Pod contract. These are configuration requirements, not a claim of validation against a production cluster. See [Scheduled operations](OPERATIONS.md).

## Optional build-network CA

The default build uses the builder's public CA roots. Networks that intercept TLS may supply a reviewed public CA certificate or bundle through a build secret:

```console
docker build --secret id=agentctl_ca,src=/protected/path/build-ca.pem \
  --tag agentctl:local --file Containerfile .
```

For the repository acceptance wrapper, set `AGENTCTL_BUILD_CA_FILE=/protected/path/build-ca.pem` before `cargo xtask acceptance-container`. Hosted `main` and manually dispatched runs accept the protected secret `AGENTCTL_BUILD_CA_PEM`, materialize it only in the runner's temporary directory, and remove it after the build. Pull-request runs never receive that secret.

The `Containerfile` combines the secret with public roots on a tmpfs mount for the single Cargo build step. The CA value is not a build argument, image environment value, build-context file, layer, history value, runtime file, or artifact. Never use `--insecure`, `CARGO_HTTP_CHECK_REVOKE=false`, a TLS-verification disable flag, or a committed certificate.

Runtime TLS interception is separate from build TLS. Mount a reviewed
certificate-only PEM bundle read-only, authorize its parent under
`secretFileRoots`, and reference it through `policy.network.customCa`:

```yaml
spec:
  policy:
    secretFileRoots: [/run/agentctl-ca]
    networkAllowlist: [api.internal.example]
    network:
      allowedSchemes: [https]
      allowedPorts: [443]
      customCa: { file: /run/agentctl-ca/runtime-ca.pem }
```

The adapter adds the bundle to rustls in memory. The bundle is not copied into
SQLite, effects, traces, or artifact storage. Invalid, empty, private-key, or
mixed-object PEM input fails before dispatch. See [Network
policy](guides/NETWORK_POLICY.md).

## Validation level

Check evidence for the exact source, image flavor, platform and digest you intend to use. A successful local engine run proves that invocation on that engine; it does not establish execution on every CI vendor, cluster or processor architecture. The release workflow retains build, scan, SBOM, smoke-test and source/artifact relationships for review. Publication remains a separate maintainer action.

The first workflow above is credential-free. The independent remediation demo uses actual Trivy evidence and separately bounded agent calls; its recorded evidence must identify which services and model IDs were executed. Offline replay covers recorded agentctl execution, not fresh outer build, scan or PR effects. Historical validation records remain available in Git and are not evidence for an untested later image.
