# Provider portability

## Problem

A workflow author wants one provider-neutral agent contract while retaining honest capability differences and evidence levels.

## Why agentctl fits

The core stores provider-neutral messages, tools, usage, errors, and continuation. Native adapters translate at the edge, and the compiler rejects requested features that the selected provider does not support.

## Credential-free variant

Source: `examples/docs/provider-portability/fake.yaml`.

<!-- agentctl-include: examples/docs/provider-portability/fake.yaml language=yaml -->

Run it with:

```text
agentctl run examples/docs/provider-portability/fake.yaml \
  --db /tmp/provider-fake.db --output json --color never
```

Expected summary: `PORTABLE_SUMMARY_VERIFIED`.

## Opt-in OpenAI variant

Source: `examples/docs/provider-portability/openai.yaml`.

<!-- agentctl-include: examples/docs/provider-portability/openai.yaml language=yaml -->

Static validation needs no credential:

```text
agentctl check examples/docs/provider-portability/openai.yaml
agentctl providers inspect examples/docs/provider-portability/openai.yaml
```

Execution requires `OPENAI_API_KEY` in the environment, makes a paid network request, and is excluded from normal documentation verification.

## State and security

Both workflows share the provider-neutral agent shape. Each provider still needs its native credential, allowed host, model name, capability checks, timeouts, and error handling.

## Current limitation

Provider portability does not mean identical behavior or equal maturity. Fake is deterministic, OpenAI has retained bounded live evidence, and Azure OpenAI, Anthropic, and Google are mock-protocol tested only in this release.
