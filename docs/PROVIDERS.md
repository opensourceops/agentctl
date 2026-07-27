# Providers

The core defines provider-neutral messages, text/reasoning/tool content, strict tool schemas, tool calls/results, finish reasons, usage, and opaque continuation. The compiler compares each agent’s requested structured output, tools, reasoning, cache, and continuation needs with typed provider capabilities.

| Kind | Native API | Implemented behavior | Credential default |
| --- | --- | --- | --- |
| `fake` | in-process scripted provider | deterministic echo/script, tool path, usage, typed streaming | none |
| `openai` | Responses API | GPT-5.6; strict function tools and structured output; multiple call IDs; stored and stateless continuation; reasoning effort/mode/context; prompt-cache mode/TTL; input/output/reasoning/cache metrics; typed SSE streaming | `OPENAI_API_KEY` |
| `azure_openai` | Azure `/openai/v1/responses?api-version=v1` | OpenAI mapping and SSE with Azure `api-key`; explicit endpoint required | `AZURE_OPENAI_API_KEY` |
| `anthropic` | Messages API | native content/tool/thinking blocks, structured output instruction, usage and stop mapping | `ANTHROPIC_API_KEY` |
| `google` | Gemini `generateContent` | native contents/function declarations/calls/results, thought-signature continuation, response schema, token usage | `GEMINI_API_KEY` |

Endpoints must pass the workflow scheme, host, effective-port, and resolved-IP
policy. Every DNS answer must be allowed, and direct clients pin the accepted
answer to prevent resolution drift. Private addresses and environment proxies
are denied by default. Redirects and Unix-socket transports are disabled.
Response bytes and DNS/connect time are bounded, composed with the provider
task timeout and the adapter's lower hard limit. See [Network
policy](guides/NETWORK_POLICY.md).
Credentials and configured headers accept environment, mounted-file, or
policy-gated process references. Provider credentials in the fresh execution
closure are preflighted before a new run record or effect; custom headers
resolve while building a required adapter.
Standard authentication headers override custom headers. Successful/error
response JSON keys and values plus provider request IDs are scrubbed of
configured secrets before parsing or persistence. Calls honor timeout and
cancellation. See [Secret references](guides/SECRET_REFERENCES.md).

`agentctl providers inspect <workflow>` reports declared capabilities without calling a service. OpenAI has the broadest mock request/response/tool/usage/error coverage. Azure OpenAI, Anthropic, and Google have native mapping and focused mock-protocol coverage at the maturity shown below; normal tests have no credentials. Live provider workflow examples end in `-live.yaml` and are opt-in.

| Provider | Validation level in this tree |
| --- | --- |
| Fake | deterministic in-process runtime and acceptance tested |
| OpenAI | native adapter mock-protocol tested; prior bounded GPT-5.6 tool workflow live-tested |
| Azure OpenAI | native adapter request/auth/response mapping mock-tested; not live-tested |
| Anthropic | native text/tool/usage mapping mock-tested; not live-tested |
| Google | native text/function/usage mapping mock-tested; not live-tested |

`agentctl providers smoke-openai --live --model gpt-5.6` remains a provider-only diagnostic; it is not runtime acceptance. The repository-owned live gate is `cargo xtask acceptance-live-openai`. It runs a YAML workflow through compilation, SQLite, a real strict function call, built-in tool policy/schema validation, stateless encrypted-reasoning continuation, deterministic assertion/artifact creation, public inspection, and replay with the credential removed. It repeats the journey inside the production OCI image and never runs in normal CI. Anthropic, Google, and Azure are implemented and mock-tested but are not live-tested in this release.

`cargo xtask examples-verify-live-openai` is the broader opt-in gate. It runs every public OpenAI workflow plus the canonical two-agent repair. A repaired task starts a new Responses session. The failed source task's response ID, stateless continuation items, pending tool call, and reasoning state are not copied. Validated task output is the cross-run dataflow boundary.

`cargo xtask resource-budget-live-openai` is the narrow resource-control gate. It
allows one real `gpt-5.6` dispatch, durably denies the second requested model
effect, and verifies the provider-request ledger through public inspection.

OpenAI provider options are an allowlisted map (`store`, `reasoningContext`, `promptCacheMode`, `promptCacheTtl`, `parallelToolCalls`, and `safetyIdentifier`). Unknown options or invalid values fail compilation. Tool-using OpenAI and Azure OpenAI agents may set `store: false`; the adapter requests encrypted reasoning content and replays the complete ordered response-item and function-output history. `stream: true` selects typed Responses SSE for fake, OpenAI, and Azure OpenAI agents. Anthropic and Google streaming fail capability negotiation. Programmatic tool calling remains unsupported and fails rather than being ignored. Parallel function calls are parsed and correlated, but one agent task executes them serially in response order. Independent workflow tasks can use bounded parallel scheduling.

## Stateful and stateless continuation

With the default `store: true`, OpenAI continuation uses `previous_response_id`.
The next request sends only the latest function outputs and relies on the
provider-stored response.

With `store: false`, agentctl does not send `previous_response_id`. It
automatically requests `reasoning.encrypted_content`, normalizes each returned
reasoning, message, and function-call item into the provider-neutral
continuation, and replays those items in their original order with correlated
function outputs, following the official [stateless encrypted-reasoning
contract](https://developers.openai.com/api/reference/resources/responses/methods/create).
A returned reasoning item without encrypted content fails closed instead of
silently losing model context.

Both modes persist continuation under provider-session format version 1.
Stateless request input is capped at 8 MiB before dispatch. Provider responses
remain subject to the configured response-byte limit and the 4 MiB adapter hard
limit. Continuation and model-effect payloads use the selected-field
state-encryption boundary when state encryption is enabled. Repair starts a
fresh provider session, while pause/resume restores the task-local continuation
and recorded replay performs no provider dispatch.

Cost is not inferred when a provider returns no reliable cost metadata.
Agent `maxCostUsd` and run `maxCostMicrousd` can use explicit versioned custom
pricing keyed by `provider/model`; otherwise a requested monetary limit fails
capability negotiation. Token-only run budgets remain enforceable from native
usage without pricing. Retry is limited to explicit task bounds and definitive
retryable HTTP responses. Timeout, cancellation, or a transport loss after
dispatch is considered ambiguous and is not automatically reissued. See
[Resource and cost budgets](guides/RESOURCE_BUDGETS.md).

Streaming persists each accepted fragment before reading more transport data.
Records are bounded and redacted, while the terminal response still follows
the normal validation path. See [Durable provider
streaming](guides/DURABLE_STREAMING.md).
