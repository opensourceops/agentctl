# Secret references

Workflow YAML stores references, never resolved secret values. `agentctl`
supports three reference forms:

```yaml
credential: { env: OPENAI_API_KEY }
credential: { file: /run/secrets/openai }
credential:
  process:
    command: /usr/local/bin/secret-helper
    args: [read, openai]
    timeoutSeconds: 5
    outputLimitBytes: 16384
```

Existing `{ env: NAME }` documents remain compatible. Inline secret strings,
secret-valued CLI arguments, and automatic external secret-manager adapters are
not supported.

## Environment references

An environment reference must use a valid variable name. Primary provider
credentials keep the established convention of `OPENAI_API_KEY`,
`AZURE_OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY` and do not
require a duplicate `policy.environmentAllowlist` entry. Environment
references used for custom HTTP headers or action environment values must be
listed in `environmentAllowlist`.

Only providers reached by an agent task in the execution closure are
credential-preflighted. Resolution occurs before a fresh run record or effect
is created. Resume, retry, and repair derive that closure from durable task
state or the accepted plan, so a reused provider task does not require its
credential. An unused declared provider does not make an otherwise
deterministic workflow require a credential.

## Mounted-file references

File references require at least one `policy.secretFileRoots` entry:

```yaml
spec:
  providers:
    openai:
      kind: openai
      credential: { file: /run/secrets/openai }
  policy:
    secretFileRoots: [/run/secrets]
    networkAllowlist: [api.openai.com]
```

Relative references and roots are resolved from `policy.workspaceRoot`.
Absolute roots support container secret mounts. The resolved target must be an
existing regular file whose canonical path remains inside a canonical allowed
root. A symlink contained by the root is accepted, which supports projected
secret rotation. A symlink escape, directory, missing path, or `..` traversal
is rejected.

Reads are limited to 64 KiB and must be UTF-8. Exactly one trailing LF or CRLF
is removed to support ordinary secret files. Empty and oversized values fail
closed.

## Process references

A process reference is disabled unless its executable basename appears in
`policy.secretProcessAllowlist`:

```yaml
spec:
  policy:
    secretProcessAllowlist: [secret-helper]
  providers:
    openai:
      kind: openai
      credential:
        process:
          command: /usr/local/bin/secret-helper
          args: [read, openai]
          timeoutSeconds: 5
          outputLimitBytes: 16384
```

The helper is invoked directly with the declared argument vector. No shell is
inserted. It starts in the policy workspace with a cleared environment, belongs
to a terminable process group where supported, and receives the run
cancellation token. `timeoutSeconds` defaults to 5 and is limited to 60.
`outputLimitBytes` defaults to 16 KiB and is limited to 64 KiB. Standard error
is bounded separately and never becomes the secret or an error message.
Nonzero exit, timeout, cancellation, non-UTF-8 output, empty output, and output
overflow all fail closed.

Process references are useful for an already installed, reviewed credential
helper. They do not turn workflow policy into an operating-system sandbox. An
allowed helper runs with the `agentctl` process identity.

## Container-mounted secret

Mount the secret read-only and grant only its parent directory:

```console
docker run --rm --read-only --user 65532:65532 \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --mount type=bind,src="$PWD/config",dst=/config,readonly \
  --mount type=bind,src="$PWD/workspace",dst=/workspace,readonly \
  --mount type=bind,src="$PWD/state",dst=/state \
  --mount type=bind,src="$PWD/openai.key",dst=/run/secrets/openai,readonly \
  ghcr.io/OWNER/agentctl:0.2.0 \
  run /config/workflow.yaml --workspace /workspace \
  --db /state/runtime.db --output json --color never
```

Kubernetes projected Secrets and Docker or Compose secrets can use the same
`/run/secrets` workflow contract. Do not copy a secret into the image or state
mount.

## Resolution, redaction, and persistence

Provider credentials are preflighted for reachable provider tasks, protocol
headers are resolved while the execution registry is built, and action
environment references are resolved at their task boundary. Resolved values
are held in zeroizing in-memory wrappers. File and process resolution uses the
same cancellation token as the run.

Only the safe source description and a SHA-256 value digest enter an action
effect record. Provider/protocol responses and subprocess output redact every
resolved value before output, tracing, or persistence. Inspection never
returns a resolved value:

```console
agentctl auth check workflow.yaml --output json
agentctl providers inspect workflow.yaml --output json
```

`auth check` tests environment presence and file availability without reading
their values. It deliberately does not execute a process reference.

Redaction protects accidental echoes, not deliberate exfiltration. Any
authorized provider, protocol peer, helper, or subprocess that receives a
secret can transform or transmit it. Use reviewed workflows, least-privilege
credentials, container isolation, and egress controls.
