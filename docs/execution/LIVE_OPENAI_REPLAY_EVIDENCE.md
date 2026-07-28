# Live OpenAI durable-replay evidence

Evidence date: 2026-07-22 (Asia/Kolkata)

Status: **passed**. A packaged macOS arm64 `agentctl` executed the canonical GPT-5.6 tool workflow, retained its exact SQLite state, and the updated production Linux arm64 image replayed a byte-identical copy of that state as UID/GID 65532 with no credential and `--network none`.

This document preserves the original stateful evidence. The canonical workflow
now uses `store: false`; the 2026-07-27 packaged stateless run and keyless replay
are recorded in [VERIFICATION.md](VERIFICATION.md#stateless-openai-follow-up).

The local evidence is ignored at `.release-evidence/openai-live/`. It contains the exact pre-replay database, the post-replay database, the generated artifact, machine-readable public-CLI output, safe metadata, a manifest, and sanitized commands. It is deliberately not committed because normalized prompts, provider results, tool output, and workspace content are durable runtime data.

## Live execution

At evidence time, the canonical workflow was
`examples/openai-live/workflow.yaml`. It used GPT-5.6, one strict
model-selected `builtin.workspace.read` call, a concrete fixture marker, an
exact final verdict, a deterministic assertion, and a deterministic artifact
write. OpenAI tool continuation used the documented `previous_response_id`
plus function-call output flow described by the official [function-calling
guide](https://developers.openai.com/api/docs/guides/function-calling).

The final retained invocation was:

```console
dist/agentctl-0.2.0-aarch64-apple-darwin/agentctl run \
  .release-evidence/openai-live/workspace/workflow.yaml \
  --workspace .release-evidence/openai-live/workspace \
  --db .release-evidence/openai-live/state-final/runtime.db \
  --output json --color never --timeout-seconds 120
```

After the compliant run succeeded, `state-final` was promoted to the retained canonical `state` directory without changing the database bytes.

The credential was inherited only by the process. Its value was not a command argument, YAML value, output field, database value, container environment, or committed artifact.

| Field | Final retained value |
| --- | --- |
| Run ID | `run-019f89f5-bdc4-70a1-83bf-b3327388b4eb` |
| Trace ID | `trace-019f89f5-bdc4-70a1-83bf-b347e8def8a1` |
| State | `succeeded` |
| Model | `gpt-5.6` |
| Provider requests | 2 |
| Model-selected tool calls | 1 (`read_fixture`) |
| Usage | 530 input, 33 output, 0 reasoning, 0 cache-read, 0 cache-write tokens |
| Provider-reported cost | unavailable (`costMicrousd: null`) |
| Checkpoints / audit / trace records | 11 / 15 / 12 |
| Workflow digest | `aea4a9441129de8a3e0c7c7ad8061af58727816d1c11cd2799b63a97e14553a6` |
| Plan digest | `aef56e9a102a711ecc7d8e35f85185bedc878db8983acf118caf28f7b7c63bb8` |

Public `inspect` reported four confirmed successful effects: two `model` effects, one `observe` tool effect, and one `workspace_mutate` artifact effect. It also reported one successful tool call and one persisted OpenAI continuation session.

This task made two bounded live workflow executions. The first exposed that the canonical YAML's redundant explicit credential-environment reference was persisted with the workflow. The reference contained no key value, but it violated this gate's stricter database rule, so the example now relies on the CLI's built-in OpenAI credential default. The final compliant run used two more provider requests. Task total: **4 provider requests**, 1,060 input tokens, 66 output tokens, and no reasoning/cache tokens. No further live call was made.

## Credential-free network-disabled replay

Before replay, an empty-environment `auth check` reported the OpenAI credential as absent. The container received no credential file or environment variable; image defaults contain only `PATH` and the CA-certificate location.

The host-created SQLite file was copied byte-for-byte into a container-managed volume because macOS bind-mount ownership cannot represent container UID 65532 on that existing file. The pre-replay host and volume digests matched. The public replay command was:

```console
podman run --rm --network none --read-only --user 65532:65532 \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --mount type=volume,source=agentctl-openai-live-replay-final-019f89f5,target=/state \
  agentctl-acceptance:local replay \
  run-019f89f5-bdc4-70a1-83bf-b3327388b4eb \
  --db /state/runtime.db --output json --color never
```

| Field | Replay value |
| --- | --- |
| Replay run ID | `replay-019f89f6-5daa-73e0-bea0-ccd55b3ee5ac` |
| Replay trace ID | `trace-019f89f6-5daa-73e0-bea0-cce2288704a4` |
| Source link | `parentRunId = run-019f89f5-bdc4-70a1-83bf-b3327388b4eb` |
| State / mode | `succeeded` / `replay` |
| Fresh effects / model effects / tool calls / provider sessions | 0 / 0 / 0 / 0 |
| Network | `none` |
| Credential forwarded | no |
| Exit / stderr bytes | 0 / 0 |

Replay public inspection contains a `replay.effects_reused` audit event on the replay trace. It links the source run, all four original effect IDs and statuses, and the original successful tool-call/effect pair. The replay itself creates no effect or tool-call rows.

The deterministic regression `recorded_replay_never_calls_provider_or_tool_executor` now replays against provider and tool executors that panic if invoked. It proves identical output, zero replay effects/tool calls, and exact source-effect/tool-call audit references.

## Digests and comparisons

| Artifact | SHA-256 |
| --- | --- |
| Runtime database before replay | `8920ab40656e0b4258e89bfdacef0d243c64cb5bf496ab686c9633df861ff621` |
| Runtime database after replay | `e4a6c68892bfb992e02a48c46e0bc2b4601ee1e414b928517f890690d202d2c3` |
| Artifact before replay | `e8d13b658dad59fdf7914765dfc79541bf1765a78cab522967b183b3038e9667` |
| Artifact after replay | `e8d13b658dad59fdf7914765dfc79541bf1765a78cab522967b183b3038e9667` |
| Live stdout | `343cd889cdba2a8108b4f8cc1ffe55fcb4ba8e1782911aae2d2e446b0505652d` |
| Replay stdout | `291dfccbabea59fe97f9e20914ac2d6cd3992f019adc17b9ad48428db3b60f37` |
| Canonical semantic `/data/output` | `670d0705bfe2a81c6c6cdbb4c7ca91428c28ac51e5de5fff2a407fa55a0f9f5b` |

The database digest changes because replay durably records a distinct run, task transitions, checkpoints, and audit records. Live and replay envelopes have different operational IDs, but their canonical declared outputs have the same digest. Recorded replay intentionally does not execute the artifact write again; the workspace was not mounted into the replay container, and the original declared artifact digest remained unchanged before and after replay.

## Security inspection and local review

- Exact key and every 16-byte key fragment: zero matches across retained evidence.
- Database credential names, authorization markers, and bearer markers: zero matches in both databases.
- Generic key/authorization patterns: zero matches.
- Unexpected host paths: zero; only the explicit ignored workspace base path is present.
- Image configuration/history exact-key or fragment matches: zero.
- Static, live, replay, and inspect stderr: zero bytes for the final successful journey.
- Durable provider content is the normalized effect input/result required for inspection and replay, not a raw HTTP response, header set, or environment dump.

Start local review with `.release-evidence/openai-live/manifest.json` and `.release-evidence/openai-live/commands.txt`, then use the packaged public CLI:

```console
dist/agentctl-0.2.0-aarch64-apple-darwin/agentctl inspect \
  run-019f89f5-bdc4-70a1-83bf-b3327388b4eb \
  --db .release-evidence/openai-live/state/runtime.db \
  --output json --color never

dist/agentctl-0.2.0-aarch64-apple-darwin/agentctl inspect \
  replay-019f89f6-5daa-73e0-bea0-ccd55b3ee5ac \
  --db .release-evidence/openai-live/state/runtime-after-replay.db \
  --output json --color never
```

Protect the ignored directory like runtime state and remove it under the team's evidence-retention policy after review.
