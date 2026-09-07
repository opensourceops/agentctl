# Environment variables and default paths

## Provider credentials

| Provider kind | Conventional reference | Required when |
| --- | --- | --- |
| `fake` | none | Never. |
| `openai` | `OPENAI_API_KEY` | The workflow dispatches an OpenAI request. |
| `azure_openai` | `AZURE_OPENAI_API_KEY` | The workflow dispatches an Azure OpenAI request. |
| `anthropic` | `ANTHROPIC_API_KEY` | The workflow dispatches an Anthropic request. |
| `google` | `GEMINI_API_KEY` | The workflow dispatches a Google request. |

These names are defaults used by repository examples. A workflow can name
another valid environment reference or use a mounted-file or policy-gated
process reference. Primary provider credential environment names do not require
a duplicate environment allowlist entry; custom headers and action environment
values do. Values never belong in YAML, CLI arguments, ordinary inputs, logs,
or committed fixtures. See [Secret references](../guides/SECRET_REFERENCES.md).

## State-encryption keys

State encryption accepts an environment-variable reference through `--key-env`. The referenced value must be base64 for exactly 32 bytes. The database stores the key ID and environment-variable name, never the value. Once enabled, every command that opens that database must receive the current reference. Rotation also needs the new reference for that command.

## Repository and acceptance variables

| Variable | Scope | Purpose |
| --- | --- | --- |
| `AGENTCTL_CONTAINER_ENGINE` | local acceptance | Select `docker` or `podman` when auto-detection is unsuitable. |
| `AGENTCTL_BUILD_CA_FILE` | local container build | Path to a reviewed CA bundle supplied as a build secret. |
| `AGENTCTL_BUILD_CA_PEM` | non-PR hosted container workflow | Protected secret materialized temporarily by `main` or manually dispatched CI; pull-request runs never receive it. |

Normal `cargo xtask docs-verify`, `cargo xtask verify`, and `cargo xtask acceptance` need no provider credential.

## CLI paths

| Path | Default or contract | Notes |
| --- | --- | --- |
| Workflow file | positional argument | Read-only input, at most 1 MiB. |
| Workspace | current directory | Override with `--workspace`. |
| Runtime database | `.agentctl/runtime.db` | Override with `--db`; SQLite WAL belongs to the same state set. |
| CAS artifact root | `<database-parent>/artifacts` | Immutable SHA-256 blobs; back up with SQLite. |
| Workflow output path | workflow-defined | Must remain under a policy-approved writable root; successful bounded files are ingested into CAS. |

## Container paths

| Path | Access |
| --- | --- |
| `/workspace/config` | reviewed read-only workflow, instruction, variable and pack files inside the workspace read boundary |
| `/workspace` | normally read-only workspace |
| `/state` | writable SQLite and content-addressed durable state |
| `/artifacts` | writable workflow output/export mount explicitly granted through `writableRoots` |
| `/run/secrets` | optional read-only mounted secret files granted through `secretFileRoots` |
| `/tmp` | small runtime tmpfs when the root filesystem is read-only |

Both image flavors default to UID/GID 65532. Provision state and artifacts for that identity, or explicitly select the matching non-root host UID/GID when bind-mounting host-owned directories. The minimal image has no shell or process tooling; the tooling flavor adds a shell, Python and Git. See the [container walkthrough](../CONTAINER.md) for complete commands.
