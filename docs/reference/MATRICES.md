# Capability matrices

## Provider capabilities and evidence

| Provider | Text | Tools | Structured output | Reasoning | Continuation | Current evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Fake | yes | scripted | deterministic fixture behavior | no | scripted | deterministic runtime and acceptance |
| OpenAI | yes | strict function tools | native response format | supported options | stored Responses continuation | mock protocol plus retained bounded live GPT-5.6 evidence |
| Azure OpenAI | yes | strict function tools | OpenAI response mapping | supported options | stored Responses continuation | mock protocol only |
| Anthropic | yes | native tool blocks | instruction mapping | native thinking blocks | provider-neutral continuation | mock protocol only |
| Google Gemini | yes | native function calls | response schema | thought-signature mapping | provider-neutral continuation | mock protocol only |

Capabilities are negotiated from source metadata before execution. `agentctl providers inspect WORKFLOW` reports what the selected configuration requests and supports without making a service call. Provider features are not assumed equivalent.

## Built-in tool executors

| Tool kind | Capability | Effect class | Typical idempotency | Policy boundary |
| --- | --- | --- | --- | --- |
| `builtin.workspace.read` | `filesystem.read` | observe | idempotent | canonical workspace root |
| `builtin.workspace.write` | `filesystem.write` | workspace mutation | keyed or at-most-once by contract | approved writable roots |
| `builtin.echo` | declared echo capability | pure | pure | tool allowlist and schema |

The compiler verifies that a built-in tool declaration matches its executor semantics. A model cannot change capability, risk, effect class, idempotency, or approval behavior.

## Built-in actions

| Action kind | External interaction | Replay behavior |
| --- | --- | --- |
| `builtin.assign` | none | recorded output |
| `builtin.assert` | none | recorded terminal result |
| `builtin.read` | workspace observation | recorded result, no fresh read in replay |
| `builtin.write` | workspace mutation | recorded result, no fresh write in replay |
| `builtin.shell.exec` | direct process | recorded result, no fresh process in replay |
| working-memory read/write | run-local state | checkpointed and reconstructed |
| long-term-memory read/write | SQLite cross-run state | recorded result; long-term store is not rolled back |
| `mcp.call` | remote tool | recorded result, no fresh call in replay |
| `a2a.delegate` | remote agent | recorded result, no fresh delegation in replay |

See [Providers](../PROVIDERS.md), [Tools](../TOOLS.md), and [Durable execution](../DURABLE_EXECUTION.md).
