# Environment variables and default paths

## Provider credentials

| Provider kind | Conventional reference | Required when |
| --- | --- | --- |
| `fake` | none | Never. |
| `openai` | `OPENAI_API_KEY` | The workflow dispatches an OpenAI request. |
| `azure_openai` | `AZURE_OPENAI_API_KEY` | The workflow dispatches an Azure OpenAI request. |
| `anthropic` | `ANTHROPIC_API_KEY` | The workflow dispatches an Anthropic request. |
| `google` | `GEMINI_API_KEY` | The workflow dispatches a Google request. |

These names are defaults used by repository examples. A workflow can name another valid environment reference. Policy must allow the name. Values never belong in YAML, CLI arguments, ordinary inputs, logs, or committed fixtures.

## State-encryption keys

State encryption accepts an environment-variable reference through `--key-env`. The referenced value must be base64 for exactly 32 bytes. The database stores the key ID and environment-variable name, never the value. Once enabled, every command that opens that database must receive the current reference. Rotation also needs the new reference for that command.

## Repository and acceptance variables

| Variable | Scope | Purpose |
| --- | --- | --- |
| `AGENTCTL_CONTAINER_ENGINE` | local acceptance | Select `docker` or `podman` when auto-detection is unsuitable. |
| `AGENTCTL_BUILD_CA_FILE` | local container build | Path to a reviewed CA bundle supplied as a build secret. |
| `AGENTCTL_BUILD_CA_PEM` | hosted container workflow | Protected secret materialized temporarily by CI. |

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
| `/config` | reviewed read-only configuration |
| `/workspace` | normally read-only workspace |
| `/state` | writable SQLite and content-addressed durable state |
| `/artifacts` | writable workflow output/export mount |
| `/tmp` | small runtime tmpfs when the root filesystem is read-only |

State and artifacts must be writable by UID/GID 65532 in the production image.
