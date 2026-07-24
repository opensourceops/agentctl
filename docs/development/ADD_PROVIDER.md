# Add a provider

Provider support means a native adapter with explicit capabilities and deterministic protocol evidence. A provider name alone is not support.

## Capability metadata

Declare support for text, structured output, tools, reasoning, continuation, cache options, usage fields, and limits. The compiler must reject any requested feature the adapter cannot honor.

## Authentication and network boundary

Use the core secret-reference contract and runtime resolver. Resolve credentials
only at the adapter boundary, never from a CLI key flag. Enforce the reviewed
endpoint host, disable redirects, use rustls, and define whether an endpoint
override is permitted.

## Native request mapping

Map provider-neutral messages, tool definitions, tool results, structured output, reasoning, and continuation into the provider's native API. Do not route a native API through an assumed OpenAI-compatible shape.

## Response and usage mapping

Parse every supported content block, tool call ID, finish reason, continuation token, request ID, and usage counter. Validate untrusted JSON and bound response reads. Do not invent monetary cost when the provider does not return authoritative cost data.

## Timeouts, cancellation, and errors

Normalize status, authentication, rate limit, malformed response, timeout, cancellation, and transport errors. Retry only definitive retryable responses within the task bound. Treat loss after dispatch as ambiguous when the provider may have acted.

## Tool continuation

Preserve correlation across model response, tool call, durable effect, tool result, and the next provider request. Document whether continuation depends on stored provider responses or stateless replayed items.

## Evidence

Add local mock-protocol tests for request mapping, authentication, headers, redirects, errors, usage, tool continuation, cancellation, redaction, and response bounds. Add provider conformance fixtures without credentials. Live validation must be an opt-in bounded gate with retained sanitized evidence and must never become a normal CI requirement.

## Documentation claims

Update the provider guide and matrix with the exact level: implemented, mock-protocol tested, retained live evidence, or deferred. Add an example that passes `check` without resolving a secret. Run `cargo xtask generate`, `cargo xtask docs-verify`, and `cargo xtask verify`.
