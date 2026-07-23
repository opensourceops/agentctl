# Add an action or tool

An action is selected directly by a task. A tool is requested by a model. Both need explicit data, policy, effect, persistence, replay, and test contracts.

## 1. Define the contract

Add the DSL kind and strict fields in `agentctl-core`. Deny unknown fields. Define the input and output shape, size and timeout bounds, and diagnostics for invalid configuration.

For a model-callable tool, require strict JSON Schema with `additionalProperties: false` where appropriate. Add capability, risk, effect class, idempotency, retry-safety, approval, secret, and network metadata.

## 2. Classify the effect

Choose the narrowest honest class. Pure computation needs no external guarantee. Reads are observations, writes are mutations, processes are process execution, and remote agents are separate from ordinary network calls.

State whether the operation is pure, idempotent, keyed, at-most-once, or unknown. Do not mark an operation retry-safe merely because retry is convenient.

## 3. Enforce policy outside the executor

Add the capability and resource checks before dispatch. A tool declaration cannot weaken global policy. Model output and remote metadata cannot grant authority.

## 4. Persist request and result

Create the effect record before invoking the implementation. Store a bounded, redacted request, stable digest, effect ID, status, confirmation, result or error, and trace correlation. Decide how a crash after dispatch becomes uncertain.

## 5. Define replay behavior

Recorded replay must use stored terminal output and call no new executor. Resume may reuse a confirmed result. Fork may perform a fresh operation. Add a regression test that panics if replay reaches the executor.

## 6. Test the boundary

Test valid input, invalid configuration, schema rejection, policy denial, approval, timeout, cancellation, output limits, redaction, persistence failure, uncertain dispatch, resume reuse, replay, and fork. Add a clean credential-free example when users need the feature.

## 7. Document the contract

Update the DSL, tool matrix, security boundary, architecture, limitations, generated schema, and site manifest as needed. Run:

```text
cargo xtask generate
cargo xtask docs-verify
cargo xtask verify
```
