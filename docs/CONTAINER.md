# OCI and CI/CD container contract

The repository `Containerfile` builds the Rust CLI in a pinned Rust 1.88 builder and copies only the optimized binary into a maintained distroless Debian runtime. The runtime has CA roots, version/source/license OCI labels, runs as `nonroot`, has a deterministic `agentctl` entrypoint, and contains no Node.js runtime, TypeScript source, credentials, workflows, or fixtures.

This whole-workflow OCI step is distinct from action-level process isolation.
An action with `isolation: container` asks the host agentctl process to invoke
a locally available digest-pinned image through Docker or Podman. That action
receives a read-only working-directory mount, no network, a read-only root,
non-root UID/GID 65532, dropped capabilities, `no-new-privileges`, a bounded
temporary filesystem, and explicit memory/CPU/PID/output/time limits. The
engine and exact image are preflighted and never fall back to host execution.
See [Process isolation](guides/PROCESS_ISOLATION.md).

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

## Mounts and inputs

| Path | Contract |
| --- | --- |
| `/config` | read-only reviewed workflow and pack configuration |
| `/workspace` | usually read-only source/fixture workspace |
| `/state` | writable SQLite database, CAS blobs, and durable recovery state |
| `/artifacts` | writable declared workflow output/export surface |

Pass workflow values with repeated `--input KEY=VALUE`, `--inputs-file`, or
`--inputs` JSON. Prefer files for large or sensitive non-provider inputs.
Provider credentials may reference a forwarded environment name or a read-only
mounted file under an explicit `secretFileRoots` policy. Never put a key in CLI
arguments, YAML, an image layer, or an ordinary input value. Before a bind-mount
run, provision `/state` and `/artifacts` host directories so UID/GID 65532 can
write them. Successful bounded workflow files are copied into
`/state/artifacts/sha256`; `/artifacts` remains the convenient CI collection
surface. Durable state may contain prompts, outputs, and artifact bytes;
protect it like a sensitive build artifact.

The image emits exactly one versioned JSON result on stdout with `--output json`; failures emit one versioned JSON error on stderr. The document includes exit status semantics, run/trace IDs, final state, and declared outputs. Progress is not mixed into stdout. Persist `/state` for later `inspect`, approval resolution, `resume`, `replay`, or `repair`.

## Verified Docker/Podman invocation

```console
docker run --rm --read-only --user 65532:65532 \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --mount type=bind,src="$PWD/config",dst=/config,readonly \
  --mount type=bind,src="$PWD/workspace",dst=/workspace,readonly \
  --mount type=bind,src="$PWD/state",dst=/state \
  --mount type=bind,src="$PWD/artifacts",dst=/artifacts \
  --env OPENAI_API_KEY \
  ghcr.io/OWNER/agentctl:0.2.0 \
  run /config/workflow.yaml --workspace /workspace --db /state/runtime.db \
  --input reportPath=/artifacts/report.txt --timeout-seconds 600 \
  --output json --color never
```

The value form `--env OPENAI_API_KEY` forwards an already protected host variable without placing its value in the command. The credential-free container acceptance uses the same command with the fake provider and without that environment variable.

For a container-native secret file, configure
`credential: { file: /run/secrets/openai }` and
`secretFileRoots: [/run/secrets]`, then replace the environment forwarding with
a read-only mount:

```console
docker run --rm --read-only --user 65532:65532 \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --mount type=bind,src="$PWD/config",dst=/config,readonly \
  --mount type=bind,src="$PWD/workspace",dst=/workspace,readonly \
  --mount type=bind,src="$PWD/state",dst=/state \
  --mount type=bind,src="$PWD/openai.key",dst=/run/secrets/openai,readonly \
  ghcr.io/OWNER/agentctl:0.2.0 \
  run /config/workflow.yaml --workspace /workspace --db /state/runtime.db \
  --output json --color never
```

The file is read at bounded credential preflight and its value is never copied
to the state mount. See [Secret references](guides/SECRET_REFERENCES.md).

For selective repair, mount the corrected workflow under `/config` and keep the source database plus its `/state/artifacts` CAS under `/state`. The original workspace output can be absent after successful ingestion. Plan without forwarding provider credentials:

```console
docker run --rm --read-only --user 65532:65532 --network none \
  --mount type=bind,src="$PWD/config",dst=/config,readonly \
  --mount type=bind,src="$PWD/workspace",dst=/workspace,readonly \
  --mount type=bind,src="$PWD/state",dst=/state \
  ghcr.io/OWNER/agentctl:0.2.0 \
  repair /config/repaired.yaml SOURCE_RUN_ID --from failed_task --plan \
  --workspace /workspace --db /state/runtime.db --output json --color never
```

The execution invocation may forward only credentials required by tasks in the fresh closure. Reused tasks do not access them. The container acceptance suite executes a credential-free repair under the same non-root, read-only-root, and mounted-state contract.

## Pipeline examples

All examples use the same image/entrypoint contract. Replace the image owner/tag and arrange the four host paths using the platform's storage mechanism. Exit `3` means approval is durably pending: retain the state directory as a protected artifact or persistent volume, resolve the approval in an operator-controlled job, and resume against that same state. Discarding the state directory makes resume impossible.

### GitHub Actions

```yaml
jobs:
  agentctl:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - run: mkdir -p .agentctl-state artifacts && chmod 0777 .agentctl-state artifacts
      - name: Run agentctl image
        env:
          OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}
        run: |
          docker run --rm --read-only --user 65532:65532 --tmpfs /tmp:rw,noexec,nosuid,size=16m \
            --mount type=bind,src="$GITHUB_WORKSPACE/config",dst=/config,readonly \
            --mount type=bind,src="$GITHUB_WORKSPACE",dst=/workspace,readonly \
            --mount type=bind,src="$GITHUB_WORKSPACE/.agentctl-state",dst=/state \
            --mount type=bind,src="$GITHUB_WORKSPACE/artifacts",dst=/artifacts \
            --env OPENAI_API_KEY ghcr.io/OWNER/agentctl:0.2.0 \
            run /config/workflow.yaml --workspace /workspace --db /state/runtime.db \
            --input reportPath=/artifacts/report.txt --timeout-seconds 600 \
            --output json --color never
      - name: Make mounted outputs collectable
        if: always()
        run: sudo chown -R "$(id -u):$(id -g)" .agentctl-state artifacts
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        if: always()
        with:
          name: agentctl-state-and-artifacts
          path: |
            .agentctl-state/
            artifacts/
          retention-days: 7
```

### GitLab CI

This syntax assumes a runner configured with Docker CLI access to the host daemon and a host-visible `$CI_PROJECT_DIR`.

```yaml
agentctl:
  image: docker:27-cli
  variables:
    AGENTCTL_IMAGE: ghcr.io/OWNER/agentctl:0.2.0
  before_script:
    - mkdir -p .agentctl-state artifacts && chmod 0777 .agentctl-state artifacts
  script:
    - >-
      docker run --rm --read-only --user 65532:65532
      --tmpfs /tmp:rw,noexec,nosuid,size=16m
      --mount type=bind,src="$CI_PROJECT_DIR/config",dst=/config,readonly
      --mount type=bind,src="$CI_PROJECT_DIR",dst=/workspace,readonly
      --mount type=bind,src="$CI_PROJECT_DIR/.agentctl-state",dst=/state
      --mount type=bind,src="$CI_PROJECT_DIR/artifacts",dst=/artifacts
      --env OPENAI_API_KEY "$AGENTCTL_IMAGE"
      run /config/workflow.yaml --workspace /workspace --db /state/runtime.db
      --input reportPath=/artifacts/report.txt --timeout-seconds 600
      --output json --color never
  after_script:
    - chown -R "$(id -u):$(id -g)" .agentctl-state artifacts
  artifacts:
    when: always
    expire_in: 7 days
    paths: [.agentctl-state/, artifacts/]
```

Configure `OPENAI_API_KEY` as a protected, masked GitLab variable. Do not write it in the YAML.

### Jenkins declarative pipeline

```groovy
pipeline {
  agent any
  stages {
    stage('agentctl') {
      steps {
        withCredentials([string(credentialsId: 'openai-api-key', variable: 'OPENAI_API_KEY')]) {
          sh '''
            mkdir -p .agentctl-state artifacts
            chmod 0777 .agentctl-state artifacts
            docker run --rm --read-only --user 65532:65532 \
              --tmpfs /tmp:rw,noexec,nosuid,size=16m \
              --mount type=bind,src="$WORKSPACE/config",dst=/config,readonly \
              --mount type=bind,src="$WORKSPACE",dst=/workspace,readonly \
              --mount type=bind,src="$WORKSPACE/.agentctl-state",dst=/state \
              --mount type=bind,src="$WORKSPACE/artifacts",dst=/artifacts \
              --env OPENAI_API_KEY ghcr.io/OWNER/agentctl:0.2.0 \
              run /config/workflow.yaml --workspace /workspace --db /state/runtime.db \
              --input reportPath=/artifacts/report.txt --timeout-seconds 600 \
              --output json --color never
          '''
        }
      }
      post {
        always {
          sh 'sudo chown -R "$(id -u):$(id -g)" .agentctl-state artifacts'
          archiveArtifacts artifacts: '.agentctl-state/**,artifacts/**', allowEmptyArchive: true
        }
      }
    }
  }
}
```

### Harness CI Run step

The runner needs Docker CLI/socket access and four workspace directories. The secret expression is injected as an environment variable and forwarded by name.

```yaml
- step:
    type: Run
    name: agentctl
    identifier: agentctl
    spec:
      image: docker:27-cli
      shell: Sh
      envVariables:
        OPENAI_API_KEY: <+secrets.getValue("openai_api_key")>
      command: |-
        mkdir -p .agentctl-state artifacts
        chmod 0777 .agentctl-state artifacts
        docker run --rm --read-only --user 65532:65532 \
          --tmpfs /tmp:rw,noexec,nosuid,size=16m \
          --mount type=bind,src=/harness/config,dst=/config,readonly \
          --mount type=bind,src=/harness,dst=/workspace,readonly \
          --mount type=bind,src=/harness/.agentctl-state,dst=/state \
          --mount type=bind,src=/harness/artifacts,dst=/artifacts \
          --env OPENAI_API_KEY ghcr.io/OWNER/agentctl:0.2.0 \
          run /config/workflow.yaml --workspace /workspace --db /state/runtime.db \
          --input reportPath=/artifacts/report.txt --timeout-seconds 600 \
          --output json --color never
```

The surrounding Harness stage must publish `/harness/.agentctl-state` and `/harness/artifacts` with its organization-approved artifact step even when this Run step exits nonzero. That vendor-specific publication configuration is intentionally not invented here; the Run step itself was documentation-reviewed, not externally dispatched.

### Kubernetes Job or CronJob

Use ConfigMaps for reviewed configuration, a PVC for `/state` when recovery
across Pods matters, a PVC or artifact uploader for `/artifacts`, and either a
Secret environment reference or a projected read-only Secret volume for
credentials. The container security context should set `runAsNonRoot`, UID/GID
65532, no privilege escalation, dropped capabilities, and a read-only root
filesystem. A CronJob should normally set `concurrencyPolicy: Forbid`; see
[Operations](OPERATIONS.md).

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: agentctl-report
spec:
  schedule: "*/15 * * * *"
  timeZone: Etc/UTC
  concurrencyPolicy: Forbid
  startingDeadlineSeconds: 300
  jobTemplate:
    spec:
      backoffLimit: 0
      activeDeadlineSeconds: 600
      template:
        spec:
          restartPolicy: Never
          securityContext:
            fsGroup: 65532
          containers:
            - name: agentctl
              image: ghcr.io/OWNER/agentctl:0.2.0
              args:
                - run
                - /config/workflow.yaml
                - --workspace
                - /workspace
                - --db
                - /state/runtime.db
                - --inputs-file
                - /config/inputs.json
                - --timeout-seconds
                - "540"
                - --output
                - json
                - --color
                - never
              env:
                - name: OPENAI_API_KEY
                  valueFrom:
                    secretKeyRef:
                      name: agentctl-provider
                      key: openai-api-key
              securityContext:
                runAsNonRoot: true
                runAsUser: 65532
                runAsGroup: 65532
                allowPrivilegeEscalation: false
                readOnlyRootFilesystem: true
                capabilities: { drop: [ALL] }
              volumeMounts:
                - { name: config, mountPath: /config, readOnly: true }
                - { name: workspace, mountPath: /workspace, readOnly: true }
                - { name: state, mountPath: /state }
                - { name: artifacts, mountPath: /artifacts }
                - { name: tmp, mountPath: /tmp }
          volumes:
            - name: config
              configMap: { name: agentctl-report }
            - name: workspace
              persistentVolumeClaim: { claimName: agentctl-workspace }
            - name: state
              persistentVolumeClaim: { claimName: agentctl-state }
            - name: artifacts
              persistentVolumeClaim: { claimName: agentctl-artifacts }
            - name: tmp
              emptyDir: { sizeLimit: 16Mi }
```

For a one-time invocation, use the same Pod template in a `batch/v1` `Job` and omit schedule/concurrency fields. Kubernetes CronJobs can occasionally create duplicate Jobs, so workflow effects still need appropriate idempotency.

## Validation level

On 2026-07-27, the current native-arm image built and passed the complete
credential-free Podman gate as non-root with a read-only root. The suite
exercised a real digest-pinned action container, a mock tool workflow, artifact
export and durable inspection, parallel ordered commit, selective repair,
missing-secret and invalid-workflow exits, SIGTERM, and recorded replay under
`--network none`. Trivy 0.72.0 with a freshly updated database found zero fixed
HIGH/CRITICAL findings and generated valid CycloneDX JSON with 11 components.
Image configuration and history scans found no credential or authorization
markers.

The exact retained GPT-5.6 live database had previously replayed with no
credential and no network, identical output and artifact digest, zero fresh
effects/tool calls/provider sessions, and explicit source-effect audit links.
GitLab, Jenkins, Harness, and Kubernetes examples remain
documentation-reviewed only. The automatic Ubuntu Linux x64 build, scan, and
SBOM job passed on the exact pull-request head and retained its image SBOM
artifact and digest.
