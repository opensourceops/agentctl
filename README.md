# agentctl

`agentctl` is a standalone prototype for a declarative autonomous agent runtime.

It provides:

- YAML playbook parsing and validation
- Internal graph compilation for task dependencies
- Deterministic module execution plus bounded agent steps
- Built-in workspace tools with guardrails: `builtin/read`, `builtin/write`, `builtin/edit`, `builtin/bash`, `builtin/grep`, `builtin/find`, `builtin/ls`
- Provider-backed agent execution with `openai.responses`, including `openai` and `azure-openai-responses`
- Provider-backed agent tools for local and remote MCP servers plus local and remote A2A peers
- Agent tool profiles: `none`, `inspect`, `workspace_write`, `workspace_exec`
- Policy enforcement for workspace roots, writable roots, and approval modes
- SQLite-backed checkpoints for replay and resume
- Audit events and trace spans with optional OpenTelemetry export hooks
- Pack manifests for reusable agents, modules, and policies

Current protocol support:

- `mcp:<server>/<tool>` routes agent tool calls through a registered MCP server transport or a remote MCP Streamable HTTP endpoint declared in the playbook
- `a2a:<agent>` routes agent tool calls through a registered A2A peer transport or a remote A2A HTTP endpoint discovered from an agent card

Playbooks can either bind concrete in-process transports at runtime or declare remote endpoints directly:

```yaml
mcpServers:
  docs:
    url: https://example.com/mcp
    bearerTokenEnv: DOCS_TOKEN

a2aAgents:
  helper:
    cardUrl: https://example.com/.well-known/agent-card.json
```

Runtime-bound transports still override playbook-declared remotes when both are provided.

<!-- CLI_REFERENCE:START -->
## CLI reference

### Top-level help

```text
Usage:
  agentctl check <playbook.yaml> [flags]
  agentctl run <playbook.yaml> [flags]
  agentctl resume <playbook.yaml> <run-id> [flags]
  agentctl replay <playbook.yaml> <run-id> <checkpoint-seq> [flags]
  agentctl db stats [flags]
  agentctl approvals <subcommand> [flags]
  agentctl prompt-cache stats [flags]
  agentctl prompt-cache explain <playbook.yaml> [flags]
  agentctl memory <subcommand> [flags]
  agentctl gc [flags]
  agentctl auth check [playbook.yaml] [flags]
  agentctl schema
  agentctl update
  agentctl help
  agentctl version

Use command-specific help for examples and command-specific flags.
Examples: "agentctl run --help", "agentctl memory --help", "agentctl auth check --help".

Examples:
  agentctl run examples/hello.playbook.yaml
  agentctl db stats
  agentctl approvals list
  agentctl prompt-cache stats
  agentctl memory stats
  agentctl auth check examples/real-autonomy/mission.playbook.yaml
  agentctl check examples/prompt-file-vars/mission.playbook.yaml

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
```

### `check`

```text
Usage:
  agentctl check <playbook.yaml> [flags]

Reports YAML syntax, schema, prompt-file, template-reference, and compile errors with exact file context when available.

Examples:
  agentctl check examples/hello.playbook.yaml
  agentctl check examples/prompt-file-vars/mission.playbook.yaml --output json

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
```

### `run`

```text
Usage:
  agentctl run <playbook.yaml> [flags]

Streams checkpoint events progressively and prints the final run result.
In interactive YAML TTY mode, paused approval-gated runs prompt inline and resume automatically after approval or rejection.

Examples:
  agentctl run examples/hello.playbook.yaml
  agentctl run examples/real-autonomy/mission.playbook.yaml --db .runtime/real-autonomy.db
  agentctl run examples/hello.playbook.yaml --output json --color never

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --db path  Runtime database path (default: ~/.agentctl/runtime/runtime.db)
  --api-key key  Runtime API key override
  --provider name  Provider for --api-key (default: openai)
```

### `resume`

```text
Usage:
  agentctl resume <playbook.yaml> <run-id> [flags]

Fails fast for terminal runs and preserves already checkpointed side effects.
In interactive YAML TTY mode, pending approvals can be resolved inline before resuming execution.

Examples:
  agentctl resume examples/hello.playbook.yaml <run-id> --db ~/.agentctl/runtime/runtime.db

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --db path  Runtime database path (default: ~/.agentctl/runtime/runtime.db)
  --api-key key  Runtime API key override
  --provider name  Provider for --api-key (default: openai)
```

### `replay`

```text
Usage:
  agentctl replay <playbook.yaml> <run-id> <checkpoint-seq> [flags]

Creates a new run id and reuses the selected checkpoint snapshot as the starting state.
In interactive YAML TTY mode, replayed approval gates can be resolved inline as the new run pauses.

Examples:
  agentctl replay examples/hello.playbook.yaml <run-id> 3 --db ~/.agentctl/runtime/runtime.db

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --db path  Runtime database path (default: ~/.agentctl/runtime/runtime.db)
  --api-key key  Runtime API key override
  --provider name  Provider for --api-key (default: openai)
```

### `db`

```text
Usage:
  agentctl db stats [flags]

Read-only runtime DB inspection. Fails on a missing DB path instead of creating one.

Examples:
  agentctl db stats
  agentctl db stats --db .runtime/real-autonomy.db --output json

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --db path  Runtime database path
```

### `prompt-cache`

```text
Usage:
  agentctl prompt-cache stats [flags]
  agentctl prompt-cache explain <playbook.yaml> [flags]

Aggregates prompt-cache hit and token usage from runtime audit events.
Explain reports why prompt cache is enabled or disabled per agent before a run.
This is observability for provider-native caching, not cache content inspection.

Examples:
  agentctl prompt-cache stats
  agentctl prompt-cache stats --db .runtime/real-autonomy.db --output json
  agentctl prompt-cache stats --run-id <run-id> --verbose
  agentctl prompt-cache explain examples/prompt-cache/mission.playbook.yaml

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --db path  Runtime database path
  --agent-ref ref  Filter prompt-cache stats to one agent ref
  --run-id id  Filter prompt-cache stats to one run
  --task-id id  Filter prompt-cache stats to one task
```

### `memory`

```text
Usage:
  agentctl memory get <key> [flags]
  agentctl memory search [flags]
  agentctl memory write <key> (--value json | --string text) [flags]
  agentctl memory stats [flags]
  agentctl memory gc [flags]

Reads fail on a missing SQLite memory DB path; writes create the DB when needed.
Use "--provider mongodb-atlas" to target the Atlas adapter instead of local SQLite.

Examples:
  agentctl memory get finding --namespace memory-flow
  agentctl memory search --query restore --limit 10
  agentctl memory write finding --namespace memory-flow --string restore-drill-missing --tags readiness,audit
  agentctl memory gc --older-than-days 30 --keep-entries 100

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --provider sqlite|mongodb-atlas  Long-term memory backend
  --db path  SQLite memory DB path (default: ~/.agentctl/memory/long-term.db)
  --connection-string uri  Remote memory backend connection string
  --database name  Remote memory database name
  --collection name  Remote memory collection name
  --namespace name  Namespace filter or write target
  --limit N  Maximum matches to return
  --older-than-days N  Retention cutoff for memory gc
  --keep-entries N  Newest entries to keep during memory gc
  --value json  JSON value for memory write
  --string text  Plain string value for memory write
  --tags a,b  Comma-separated tags for memory write
```

### `gc`

```text
Usage:
  agentctl gc [flags]

Only terminal runs are deleted. Running and paused runs are preserved.

Examples:
  agentctl gc
  agentctl gc --older-than-days 7 --keep-runs 20 --output json --verbose

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --db path  Runtime database path
  --older-than-days N  Delete terminal runs older than N days (default: 30)
  --keep-runs N  Keep newest terminal runs regardless of age (default: 100)
```

### `auth`

```text
Usage:
  agentctl auth check [playbook.yaml] [flags]

Exits nonzero when any required provider auth is missing.
When a playbook is provided, only provider-backed agents in that playbook are inspected.

Examples:
  agentctl auth check --provider openai
  agentctl auth check examples/real-autonomy/mission.playbook.yaml --output json

Flags:
  -h, --help  Show help
  -v, --verbose  Show full structured output
  -V, --version  Show version
  --output yaml|json  Structured output format
  --color auto|always|never  YAML color mode
  --api-key key  Runtime API key override
  --provider name  Provider to inspect when no playbook is given
```

### `schema`

```text
Usage:
  agentctl schema
```

### `update`

```text
Usage:
  agentctl update
```
<!-- CLI_REFERENCE:END -->

## Runtime database

By default, `agentctl` stores run state in:

```text
~/.agentctl/runtime/runtime.db
```

Use `--db` to override that path for a specific command.

`run`, `resume`, and `replay` all operate on the same SQLite file unless you point them at different `--db` paths. New runs create new rows inside the same database; they do not create a new database file unless you choose a new path.

Inspect the current database with:

```bash
agentctl db stats
agentctl db stats --output json
```

`db stats` prints:

- database path
- current file size in bytes
- run counts by status
- oldest and newest run timestamps
- record counts for checkpoints, task attempts, agent turns, audit events, and trace spans
- latest run metadata

Clean up old terminal runs with:

```bash
agentctl gc
agentctl gc --older-than-days 7 --keep-runs 20
agentctl gc --output json --verbose
```

`gc` removes only terminal runs (`succeeded` and `failed`), never `running` runs. By default it:

- deletes terminal runs older than `30` days
- keeps the newest `100` terminal runs regardless of age
- vacuums the SQLite database after deletion

`gc` prints:

- the GC policy used (`olderThanDays`, `keepRuns`)
- number of deleted runs
- whether vacuum was performed
- before/after database file size
- before/after run counts
- before/after record counts
- deleted run ids in verbose mode

## Memory model

`agentctl` uses four distinct memory modes:

- `run_memory`
  - The runtime/checkpoint state for a single run.
  - Stored in the runtime DB, by default `~/.agentctl/runtime/runtime.db`.
  - Includes inputs, task state, attempts, agent sessions, checkpoints, trace/audit state, and the current working-memory snapshot.
  - This is part of replay/resume correctness and should stay local to the runtime.

- `working_memory`
  - Mutable state for the active run.
  - Checkpointed inside the runtime DB and available in templates as `memory.working`.
  - Best for facts, intermediate findings, handoff state, and deterministic per-run scratch state.

- `long_term_memory`
  - Cross-run durable knowledge.
  - Stored separately from the runtime DB.
  - Default local store path: `~/.agentctl/memory/long-term.db`.
  - Best for approved facts, indexed artifacts, and reusable operational knowledge.
  - Extension point for future external adapters such as SQL, vector, document, and graph stores.

- `prompt_cache`
  - Provider-native optimization for supported model adapters.
  - Currently implemented for `openai.responses` with provider `openai`.
  - Disabled by default and never required for correctness.
  - Custom OpenAI-compatible base URLs are disabled by default unless `promptCache.force: true` is set.

Recommended usage:

- Use `working_memory` for state that must survive retries, resume, and replay within the same run.
- Use `long_term_memory` only for cross-run knowledge that you want to keep deliberately.
- Do not treat `run_memory` as a user-facing knowledge store.
- Use `prompt_cache` only for cost and latency optimization on stable prompt prefixes.
- Do not rely on prompt caching for correctness.

### `vars` compatibility

`vars` currently remains as a compatibility mirror of `memory.working`.

That decision is intentional for now:

- old playbooks and templates that reference `vars.*` continue to work
- the canonical state should now be treated as `memory.working.*`
- new playbooks should prefer `memory.working`

Long term, `memory.working` should be the canonical surface and `vars` should be treated as compatibility-only.

### Memory CLI

Use the first-class memory commands against the standalone long-term memory DB:

```bash
agentctl memory stats
agentctl memory get finding --namespace memory-flow
agentctl memory search --query restore --limit 10
agentctl memory write finding --namespace memory-flow --string restore-drill-missing --tags readiness,audit
```

Command behavior:

- `memory write` creates the memory DB if it does not exist
- `memory get`, `memory search`, and `memory stats` fail on a missing DB path instead of silently creating one
- `--db` defaults to `~/.agentctl/memory/long-term.db`
- `--namespace` filters to a single namespace; omitted namespace searches across all namespaces for CLI reads
- `--value` accepts JSON and `--string` writes a plain string

See [docs/memory.md](docs/memory.md) for the detailed memory guide.
See [docs/prompt-cache.md](docs/prompt-cache.md) for prompt-cache support, configuration, sharing modes, and CLI stats.
See [docs/long-term-memory.md](docs/long-term-memory.md) for long-term retention, adapters, MongoDB Atlas, retrieval/promotion, and replay/resume notes.
See [docs/custom-tools.md](docs/custom-tools.md) for pack-defined custom tools, runtime requirements, and host-command wrappers.
See [docs/agent-prompts.md](docs/agent-prompts.md) for inline prompts, prompt files, task-scoped vars, agent default vars, and execution-time prompt rendering.
See [docs/agent-kinds.md](docs/agent-kinds.md) for the supported `agents.<name>.kind` values, exact fields, and when to use each one.
See [docs/profiles.md](docs/profiles.md) for the supported agent tool profiles, capability matrix, and selection guidance.
See [docs/policies.md](docs/policies.md) for the supported policy fields, path rules, approval modes, and decision flow.
See [docs/check.md](docs/check.md) for `agentctl check`, YAML syntax validation, schema validation, and prompt-template diagnostics.
See [docs/typescript.md](docs/typescript.md) for the repository TypeScript conventions.

## Provider auth

`agentctl` resolves provider auth in this order:

1. runtime override from `--api-key`
2. stored API key in `~/.agentctl/auth.json`
3. provider environment variables such as `OPENAI_API_KEY`

Use `auth check` to diagnose provider configuration before a run:

```bash
agentctl auth check --provider openai
agentctl auth check examples/real-autonomy/mission.playbook.yaml
```

`auth check` exits with status `1` if any required provider is missing auth for the inspected playbook.

The first live provider path is `openai.responses`, using the official OpenAI SDK and the Responses API.

Supported OpenAI-related auth/config today:

- `openai`
  - `OPENAI_API_KEY`
  - `OPENAI_ORG_ID`
  - `OPENAI_PROJECT_ID`
  - `OPENAI_BASE_URL`
- `azure-openai-responses`
  - `AZURE_OPENAI_API_KEY`
  - `AZURE_OPENAI_ENDPOINT`
  - `OPENAI_API_VERSION`
  - optional `OPENAI_ORG_ID`, `OPENAI_PROJECT_ID`, `OPENAI_BASE_URL`

Stored credentials in `~/.agentctl/auth.json` can be either a legacy string API key or a structured credential object:

```json
{
  "openai": {
    "type": "api_key",
    "key": "sk-...",
    "organization": "org_...",
    "project": "proj_..."
  },
  "azure-openai-responses": {
    "type": "api_key",
    "key": "azure-key",
    "endpoint": "https://example-resource.azure.openai.com",
    "apiVersion": "2024-10-01-preview"
  }
}
```

## Real example

See [examples/real-autonomy/README.md](examples/real-autonomy/README.md) for a model-backed example that inspects a fixture, writes a report, and verifies the output deterministically.

See [examples/remote-mcp-autonomy/README.md](examples/remote-mcp-autonomy/README.md) for a second example that crosses a real remote MCP HTTP boundary before persisting and verifying the report.

See [examples/custom-pack-tools/README.md](examples/custom-pack-tools/README.md) for a pack example that lets an agent call both a wrapped host command and a custom script shipped inside the pack.

See [examples/dataflow/README.md](examples/dataflow/README.md) for a deterministic example that proves scalar and structured task outputs can flow across YAML steps without losing shape.

See [examples/prompt-file-vars/README.md](examples/prompt-file-vars/README.md) for a deterministic example that proves `instructionsFile`, task-scoped vars, agent default vars, and runtime task-output interpolation.
