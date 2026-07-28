# Resource and cost budgets

Run-wide budgets are declared under `spec.runtime.budgets`. They complement
agent, task, tool, process, and protocol limits. A run without a field has no
run-wide limit for that dimension.

```yaml
spec:
  runtime:
    budgets:
      maxProviderRequests: 4
      maxTurns: 4
      maxToolCalls: 2
      maxInputTokens: 20000
      maxOutputTokens: 2000
      maxTotalTokens: 22000
      maxWallTimeSeconds: 300
      maxProcessOutputBytes: 2097152
      maxArtifactBytes: 16777216
      maxTasks: 32
      maxExpansionItems: 16
      maxLoopIterations: 8
```

Every configured value must be greater than zero. Exact equality is allowed.
The compiler rejects a plan whose task, static foreach or matrix child, or
declared loop-iteration count exceeds its corresponding limit.

## Reservation and reconciliation

SQLite is the budget coordination point. Before a fresh provider request, tool
call, subprocess, or artifact ingestion, the runtime atomically checks current
usage plus active reservations. Parallel tasks cannot both consume the same
remaining unit. A denied reservation records `budget.exceeded` and no executor
is dispatched.

Known upper bounds are reserved conservatively:

- one provider request and turn, estimated serialized input tokens, the
  agent's maximum output tokens, and configured maximum estimated cost;
- one tool call;
- the process combined-output limit, or two limits for an extension handshake
  and invocation;
- the artifact's observed file size before ingestion.

The actual provider usage, captured process bytes, and ingested artifact bytes
replace the reservation after the effect. An actual provider response can
therefore exceed an estimate and fail the task after the response is durably
recorded. The next operation is not dispatched. `maxTotalTokens` is input plus
output tokens. Reasoning and cache counters are exposed as usage classes and
are not added again to the total.

`maxWallTimeSeconds` is measured from the durable run creation timestamp.
Pause and resume do not reset it. Expiry cancels in-flight work, records the
wall-time overrun, and fails the run. A provider, tool, or process cancelled
after dispatch still retains the normal uncertain-effect rules.

## Monetary cost

Token-only budgets require no pricing metadata. A monetary limit uses integer
micro-US-dollars:

```yaml
spec:
  runtime:
    budgets:
      maxCostMicrousd: 50000
    pricing:
      version: internal-2026-07
      models:
        openai/gpt-5.6:
          inputMicrousdPerMillionTokens: 1000000
          outputMicrousdPerMillionTokens: 8000000
          reasoningMicrousdPerMillionTokens: 8000000
          cacheReadMicrousdPerMillionTokens: 100000
          cacheWriteMicrousdPerMillionTokens: 1000000
```

Pricing keys use `provider/model`. Input and output rates are required.
Reasoning defaults to the output rate; cache read and write default to the
input rate. Reservations use the highest applicable rate for each unknown
token class and round up. Reconciliation prefers authoritative provider cost
when supplied, otherwise it uses the explicit versioned rates and rounds each
class up.

`maxCostMicrousd` requires explicit pricing for every cost-limited agent. This
keeps monetary enforcement deterministic when a provider does not return
billing metadata. Pricing is user-supplied operational configuration, not a
claim that the provider's public price is current. Update the version and rates
when the billing contract changes. Without authoritative or custom pricing,
the ledger increments `unpricedProviderRequests` and only token budgets are
enforceable.

## Recovery and inspection

Reservations and reconciliations are idempotent by run and effect identity.
They survive crashes and are included in checksummed checkpoints and ordered
audit events. Retry, repair, compensation, fork, and replay each receive a new
run ledger. Reused or recorded work consumes no new provider, tool, process, or
artifact units in the derived run.

Inspect the durable snapshot:

```console
agentctl inspect RUN_ID --db .agentctl/runtime.db --output json --color never
```

The `budget` object includes limits, pricing version, planned graph counts,
actual usage, active reservations, and the first exceeded dimension. See
[`examples/v1/resource-budget.yaml`](../../examples/v1/resource-budget.yaml)
for an expected request-budget termination.

The opt-in live gate proves the same boundary with one real OpenAI request and
then verifies that the second requested effect is not dispatched:

```console
cargo xtask resource-budget-live-openai
```

It requires `OPENAI_API_KEY`, packages the production CLI, uses `gpt-5.6`,
retains only sanitized counters and the run ID in terminal output, and is never
part of credential-free CI.
