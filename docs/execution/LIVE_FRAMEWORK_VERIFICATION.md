# Live framework verification

Date: 2026-07-27, Asia/Kolkata.

Status: passed.

The packaged macOS arm64 CLI and the production Linux arm64 OCI image executed
the bounded public OpenAI matrix with model `gpt-5.6`. Every run used a clean
temporary database and workspace. Raw provider responses and databases remain
only in ignored temporary or evidence paths. No credential value or resolved
secret was written to committed content.

## Retained successful matrix

| Scenario | Requests | Tool calls | Run | Result |
| --- | ---: | ---: | --- | --- |
| Stateless tool workflow | 2 | 1 | `run-019fa39d-de49-7410-b973-d50fed711010` | exact verdict and artifact |
| Basic agent | 1 | 0 | `run-019fa39d-e8ca-7f73-b218-f105e4c16b92` | succeeded |
| Environment secret reference | 1 | 0 | `run-019fa39d-eeb6-79a1-9f65-879196c4193d` | succeeded without persisted secret |
| Provider portability | 1 | 0 | `run-019fa39d-f6b2-7a21-946a-81d4b6d5269f` | succeeded |
| Framework live composite | 11 | 0 | `run-019fa39d-feea-7ad2-9ff7-38e7bc005655` | all bounded constructs succeeded |
| Deliberate agent failure source | 3 | 2 | `run-019fa39e-3567-7bb3-a47f-496162a23d12` | failed at the designed task boundary |
| Terminal retry of that failure | 1 | 1 | `retry-019fa39e-4ce3-7773-a3d0-dcd18d46b85b` | upstream agent reused; failed task reattempted once |
| Selective repair | 2 | 1 | `repair-019fa39e-55f9-7142-b6ae-06f21a1ead8e` | upstream reused; artifact CAS reference retained |
| Linux arm64 OCI failure source | 3 | 2 | `run-019fa3a6-2fc6-7733-b201-eb806fd70425` | expected failure retained in mounted state |
| Linux arm64 OCI selective repair | 2 | 1 | `repair-019fa3a6-5b2e-7851-9a44-e3ea3e019ce0` | upstream reused and artifact exported |

The retained matrix used 27 Responses API requests, 8 tool calls, 3,939 input
tokens, 560 output tokens, 20 reasoning tokens, and no reported cache tokens.
The provider did not return billing metadata, so no billed amount is
fabricated. The gate's deliberately conservative token calculation is below
USD 0.07 for this retained matrix and its hard ceiling is USD 10.

## Composite feature proof

The public
[`live-composite.yaml`](../../examples/framework-completeness/live-composite.yaml)
compiled to 21 tasks and made 11 bounded provider requests:

- two independent agent branches executed under `maxConcurrency: 4`;
- a two-child agent matrix retained stable child IDs and ordered aggregation;
- a structured agent result selected `execute`, while the default route was
  durably skipped;
- two agent loop iterations committed and the third bounded child was skipped;
- one agent ran inside a versioned typed sub-workflow;
- collector and reviewer roles exchanged one typed durable handoff;
- one OpenAI streaming task persisted stream events and produced the exact
  final marker;
- final deterministic verification succeeded.

Replay `replay-019fa39e-34a3-71a1-a1d2-e7fcefbb66d4`
reproduced the composite output without the credential and recorded zero fresh
effects, tool calls, or provider sessions.

## Recovery, CAS, and container proof

The selective-repair source deliberately gives `publish` only one turn. It
therefore persists its tool call and fails before a final structured response.
The identical-workflow terminal retry reused `analyze`, made exactly one fresh
model attempt for `publish`, and failed again as designed. The repaired
workflow raised the bounded turn allowance. Repair then reused `analyze`,
executed only the repair closure, wrote the expected artifact, and succeeded.

Replay `replay-019fa39e-6273-7212-a60e-5519e2d88a25`
reproduced the repaired local result with zero fresh effects. The production
image repeated the source and repair path as UID/GID 65532 with read-only
configuration and workspace mounts plus writable state and artifact mounts.
Container replay
`replay-019fa3a6-7187-7481-bb8f-abed11943457`
ran with no credential and `--network none`, producing zero fresh effects,
tool calls, or provider sessions.

## Resource-budget proof

Run `run-019fa2f4-4252-7aa3-8aa8-72c31cb5dfcf` allowed exactly one
provider request. The first task succeeded with 18 input and 5 output tokens.
The second reservation was denied before dispatch, and inspection recorded
`providerRequests` with limit 1 and attempted value 2.

## Request accounting

The successful retained framework matrix used 27 requests. Two fail-closed
gate attempts were also made while implementing the evidence assertions:

- the first stopped after 20 requests because it counted the model and tool
  effects together for the terminal retry;
- the second completed all 22 local requests, then exposed that the live OCI
  block copied the credential-free repair fixture instead of the public OpenAI
  workflow;
- the focused OCI continuation used the remaining 5 requests and passed.

Including the separate one-request resource-budget proof, this limitation
burn-down used 48 live requests, below the authorized maximum of 80. The first
failed attempt's temporary database was deleted by the fail-closed harness, so
its token counts are not reconstructed. No blind retry loop or live fuzzing
was used.

Sanitized machine-readable evidence is retained at the ignored path
`.release-evidence/selective-repair/live-summary.json`.
