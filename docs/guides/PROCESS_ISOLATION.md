# Process isolation

Agentctl distinguishes process policy from operating-system isolation. An
executable allowlist decides whether an action may launch a command. It does
not restrict what an allowed command can read, write, signal, or contact with
the host identity.

`builtin.shell.exec` and `extension.process` therefore expose an explicit
`isolation` mode:

- `process` is the default host-process mode. It uses direct arguments, a
  cleared environment, explicit protected environment values, an authorized
  working directory, concurrent bounded output capture, a timeout, cancellation,
  and process-tree termination. It is not a sandbox.
- `container` is the portable stronger boundary. It requires a locally
  available content-addressed image and Docker or Podman.

Secret-provider helper processes remain bounded host processes. Use a mounted
file or environment reference when the helper itself is not trusted with the
host identity.

## Host process mode

Existing process actions keep their behavior because `process` is the default:

```yaml
spec:
  policy:
    processAllowlist: [git]
    approval: never
  actions:
    status:
      kind: builtin.shell.exec
      command: /usr/bin/git
      args: [status, --short]
      isolation: process
      timeoutSeconds: 10
```

The executable basename must appear in `processAllowlist`. `cwd` must resolve
within the readable workspace policy. The child receives no ambient
environment. Only action `env` entries authorized by policy are added.

Output and time bounds reduce accidental resource use, but the executable
still has the full filesystem, network, IPC, and operating-system authority of
the agentctl process.

## Container mode

Container mode requires a digest-pinned reference in either
`NAME@sha256:DIGEST` or local `sha256:IMAGE_ID` form:

```yaml
spec:
  policy:
    processAllowlist: [worker]
    environmentAllowlist: [WORKER_TOKEN]
    approval: never
  actions:
    isolated:
      kind: builtin.shell.exec
      command: /usr/local/bin/worker
      args: [run]
      env:
        WORKER_TOKEN: { env: WORKER_TOKEN }
      isolation: container
      container:
        image: registry.example/worker@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
        runtime: auto
        memoryLimitBytes: 268435456
        cpuLimitMillis: 1000
        pidsLimit: 64
```

The image must already exist locally. Agentctl passes `--pull=never`; workflow
execution never pulls it. `runtime` is `auto`, `docker`, or `podman`. Auto mode
tries locally available Docker and Podman engines whose image inspection
succeeds. An explicit engine never falls back to the other engine.

The invocation fixes these controls:

- read-only root filesystem;
- non-root UID/GID `65532:65532`;
- no network;
- all Linux capabilities dropped;
- `no-new-privileges`;
- read-only bind of the authorized host working directory at `/workspace`;
- writable 16 MiB `noexec`, `nosuid`, `nodev` `/tmp`;
- explicit memory, CPU, and PID bounds;
- only declared action environment values;
- direct entrypoint and arguments, with no shell;
- bounded stdout, stderr, total output, duration, and cancellation;
- forced container cleanup after timeout, cancellation, or output failure.

The image entrypoint is overridden with the action command. Action arguments
come after the image reference, so they cannot become container-engine
options. Environment values are supplied to the engine process but only their
names appear in its argument list.

`memoryLimitBytes` accepts 16 MiB through 16 GiB and defaults to 256 MiB.
`cpuLimitMillis` accepts 1 through 64,000 and defaults to 1,000, which is one
CPU. `pidsLimit` accepts 1 through 4,096 and defaults to 64.

Engine availability and exact-image inspection happen before the process
effect is requested. Missing engines, stopped daemons, and unavailable images
fail closed. The compiled plan lists every process action, its tasks, mode,
image contract, engine selection, and resource limits.

## Platform support

| Host | `process` | `container` | Native sandbox backend |
| --- | --- | --- | --- |
| Linux | host process | Docker or Podman Linux container | not implemented; no namespace or bubblewrap claim |
| macOS | host process | Docker or Podman Linux VM/container | not implemented; no macOS sandbox-profile claim |
| Windows | host process with process-tree termination | Docker or Podman when it can run the pinned image | not implemented; no restricted-token or job-object isolation claim |

Request `container` when a strong boundary is required. If the selected engine
is unavailable on the current host, agentctl reports an explicit unsupported
diagnostic instead of silently running the command as a host process.

Container isolation still exposes the mounted working directory read-only and
passes explicitly authorized secrets to the contained process. A container
engine is a privileged trusted dependency, and an authorized image can
exfiltrate data through allowed outputs. For hostile workloads, combine this
mode with a dedicated worker identity, VM boundary, externally managed image
admission, and platform resource and egress controls.
