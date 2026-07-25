# Build structured role handoffs

`agentctl` represents collaboration as an explicit workflow graph. A role is a
named bounded agent task. A handoff is a deterministic task whose typed output
is an input dependency of another role. There is no hidden team conversation,
mailbox, shared transcript, or model-owned scheduler.

Use the canonical
[`structured-handoff.yaml`](../../examples/v1/structured-handoff.yaml)
workflow as a complete example. Its graph is:

```text
collector agent -> typed handoff action -> reviewer agent -> verifier action
```

## Define roles

Each role uses an ordinary agent definition. The definition fixes its provider,
model, instructions, visible tools, maximum turns, maximum tool calls, token
limit, timeout, retry policy, and structured output contract.

Different agent definitions provide role separation. A collector can see a
read tool while a reviewer has no tools. The workflow policy remains the
non-bypassable authority for every role, and tool contracts retain their own
risk and approval requirements.

## Make the handoff explicit

The canonical handoff is a `builtin.assign` task with:

- `needs: [collect]`, which fixes the sender relationship;
- literal `from` and `to` role names;
- an exact typed template for `payload`;
- an `outputSchema` that validates the complete durable envelope.

The receiving role declares `needs: [handoff]` and reads only the validated
payload. Use a typed router task when a handoff can select one of several
explicit recipients. Every route remains a compiled graph decision.

The handoff task has an ordinary task record, attempt, output digest,
checkpoint, audit event, and trace span. No additional conversation state is
needed.

## Recovery behavior

Agent provider sessions stay task-local. Retry and selective repair can reuse a
compatible upstream role and handoff while starting a fresh downstream role
session. Cancellation follows graph dependencies. Recorded replay copies the
role and handoff records and dispatches no provider or tool effect.

If a handoff schema, payload template, role instructions, tool visibility, or
upstream output changes, the normal task fingerprint and repair closure rules
apply.

## Unsupported hidden teams

`uses: team:<name>` is rejected with migration guidance. Convert a former team
into:

1. one named agent task per bounded role;
2. one typed deterministic task per handoff;
3. explicit `needs`, conditions, and routers;
4. a reusable sub-workflow when the role graph should be invoked more than
   once.

This replacement is intentionally finite and inspectable. Free-form
multi-agent conversation is outside the product surface.
