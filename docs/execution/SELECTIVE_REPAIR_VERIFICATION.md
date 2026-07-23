# Selective repair verification

This record contains sanitized release evidence for selective workflow repair. It
does not contain provider responses, credentials, runtime databases, or prompt
transcripts.

## Baseline

Before implementation:

- `cargo xtask verify` passed.
- `cargo xtask acceptance` passed 25 credential-free scenarios.
- `cargo xtask package` passed.
- `cargo xtask acceptance-container` was blocked because the installed Podman
  engine could not connect to its Linux VM.

## Deterministic release gates

On 2026-07-23:

- `cargo xtask verify` passed all 12 stages, including formatting, Clippy with
  warnings denied, workspace tests, documentation tests, dependency policy,
  secret and action-pin scans, source installation, and the production boundary.
- `cargo xtask acceptance` passed all 28 scenarios.
- `cargo xtask examples-verify` passed the complete discovered example matrix.
- `cargo xtask docs-verify` passed all six stages.
- `cargo xtask package` produced the macOS arm64 package.

The deterministic repair coverage includes task-boundary reuse, downstream and
branch invalidation, multiple roots, changed definitions and prompt files,
output-contract and digest failures, missing artifacts, state reconstruction,
effect reconciliation, idempotency, approval gates, source immutability, source
garbage collection, migrations, transaction rollback, stable JSON output, and
effect-free offline replay.

## Live OpenAI evidence

Command:

```console
cargo xtask examples-verify-live-openai
```

Local packaged-CLI verification completed for all six OpenAI example workflows
with model `gpt-5.6`:

- Requests: 10
- Tool calls: 4
- Input tokens: 1,894
- Output tokens: 214
- Reasoning tokens: 0
- Prompt-cache read tokens: 0
- Prompt-cache write tokens: 0
- Estimated standard-tier model cost at the verified 2026-07-23 public rates:
  USD 0.01589

Selective-repair lineage:

- Source run: `run-019f8ffa-907c-7a41-944b-d6c303f898ec`
- Repair run: `repair-019f8ffa-a3c9-72a0-9a8a-2bf82e1566fb`
- Offline replay: `replay-019f8ffa-b426-78b0-b2c8-1d9b113b2824`

The live assertions proved:

- The source `analyze` task succeeded and `publish` failed after live agent and
  tool execution.
- The plan marked `analyze` reused and `publish` executable.
- The repair run contained no fresh effect, provider session, or tool call for
  `analyze`.
- `publish` used one fresh task-local provider session and one model-selected
  tool call; it did not continue the failed source session.
- The repaired output and artifact contained the persisted upstream marker
  `SELECTIVE_REPAIR_FIXTURE_CONFIRMED`, proving the repaired task consumed the
  reused result.
- The source run and source task records were unchanged after repair.
- Replay ran after removing `OPENAI_API_KEY`, preserved semantic output and the
  artifact digest, and dispatched zero effects, tool calls, or provider sessions.

The ignored local evidence file is
`.release-evidence/selective-repair/live-summary.json`.

## Blocked container gates

These commands remain blocked by the local container runtime:

```console
env -u OPENAI_API_KEY cargo xtask acceptance-container
cargo xtask examples-verify-live-openai
```

Podman 5.8.2 is installed, but its `libkrun` VM socket at
`127.0.0.1:52210` refuses connections. The live command therefore completed
the local packaged-CLI stage and stopped before building or running the OCI
stage. No container result is claimed.

Continuation:

```console
podman machine start
env -u OPENAI_API_KEY cargo xtask acceptance-container
cargo xtask examples-verify-live-openai
```

The final command would repeat the already completed local live calls before
reaching its container phase, so rerun it only when the remaining request
budget and cost are explicitly accepted.
