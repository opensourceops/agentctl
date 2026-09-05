# DevOps and CI/CD workflows

These twenty `agentctl.dev/v1` examples execute actual parsers, local patches,
tests, policy decisions and recovery commands. Every run produces structured
evidence. They use disposable local fixtures; none applies infrastructure or
deploys an external service.

From the repository checkout, run the complete credential-free suite:

```sh
cargo build -p agentctl-cli --locked
python3 examples/devops/run.py --agentctl target/debug/agentctl --report /tmp/agentctl-devops.json
```

Use `--only 01` to select an example; repeat it to select several. `--keep`
retains the fresh workspaces and prints their locations. Failed workspaces are
always retained. The runner invokes the real CLI from a separate, empty working
directory, uses explicit workflow/workspace/database paths, and records check,
plan, execution, inspection, denial, replay and relevant recovery commands.
Each JSON envelope and exit code is saved under the retained `evidence/` folder.
The aggregate report includes checkout HEAD and a dirty-tree flag, binary/workflow/helper hashes, artifact
digests, usage and assertions. A passing local report does not establish hosted
Linux/macOS/Windows evidence.

The [machine-readable catalog](catalog.json) is an exact inventory of twenty
directories, dependencies, platforms, features, modes, expected exit codes and
artifacts. The runner rejects uncatalogued/missing example directories. Static
`check` success is not execution evidence; a current suite report is required.

| # | Workflow | Observable result |
|---|---|---|
| 01 | [CI diagnosis](01-ci-diagnosis/README.md) | Parsed failure and structured advice with verified log citations |
| 02 | [JUnit triage](02-junit-triage/README.md) | Real XML parsing, assertion/infrastructure classification |
| 03 | [Pipeline review](03-pipeline-review/README.md) | Permission/timeout violations and a checked, applied Git patch |
| 04 | [Dockerfile review](04-dockerfile-review/README.md) | Narrow COPY, non-root user, code checks and separate real image gate |
| 05 | [Kubernetes review](05-kubernetes-review/README.md) | Corrected manifest checked against a bundled Deployment subset schema |
| 06 | [Terraform plan](06-terraform-plan/README.md) | Plan rules, durable approvals, isolated local-file mutation |
| 07 | [Dependency update](07-dependency-update/README.md) | Vendored patch plus failing-before/passing-after behavioral tests |
| 08 | [SBOM triage](08-sbom-triage/README.md) | Component joins, severity rules and expiring exceptions |
| 09 | [Release notes](09-release-notes/README.md) | Actual fixture Git commits and verified changed-file citations |
| 10 | [Release readiness](10-release-readiness/README.md) | Gate and package-digest evidence with an explicit no-go |
| 11 | [Configuration drift](11-configuration-drift/README.md) | External variable precedence, replacement and invocation overrides |
| 12 | [Incident timeline](12-incident-timeline/README.md) | Sorted events, duration and source-checked runbook advice |
| 13 | [Canary evaluation](13-canary-evaluation/README.md) | Both typed promotion/rollback routes with no unselected mutation |
| 14 | [Local deployment](14-local-deployment/README.md) | Durable approval and before/after probes of a disposable HTTP service |
| 15 | [Interrupted deployment](15-interrupted-deployment/README.md) | Process kill, conservative recovery and exactly one confirmed local mutation |
| 16 | [Retry and repair](16-retry-repair/README.md) | Terminal retry and selective repair reuse a successful upstream build |
| 17 | [Compensated rollout](17-compensated-rollout/README.md) | Failed health check, explicit inverse and linked reconciliation |
| 18 | [Parallel matrix](18-parallel-matrix/README.md) | Four service checks with stable ordered aggregation |
| 19 | [Role sub-workflow](19-role-subworkflow/README.md) | Typed planner/reviewer/executor handoffs and distinct tool visibility |
| 20 | [Bounded remediation](20-bounded-remediation/README.md) | Validated tool mutation, loop termination and resource/cost audit |

## Execution and authority

Python 3.10+ is required; examples 03, 04, 07 and 09 also use Git. On Windows, provide `--agentctl target/debug/agentctl.exe` or omit that option to use the platform default, and ensure `python3` is on PATH. Workflow files
use the JSON-compatible subset of YAML so the small standard-library runner
does not need a YAML dependency. The [reviewed process extension](fixture.py)
implements the versioned handshake and returns JSON; workflows validate its
outputs and write durable reports. Interpreter access is an explicit host
process grant. Host execution is not an operating-system sandbox; the helper
can invoke the documented Git/Python subprocesses. Use only reviewed helpers
on a trusted fixture runner.

The approval cases preserve `approval: mutations`. The runner lists and reviews
each pending effect, uses actor `devops-fixture-reviewer` with an explicit
reason, and resumes the same durable run. This framework version records actors
but has no actor-role authorization system: local database ownership remains a
trust boundary. The fixture does not disable policy to obtain a green result.

All cases execute a policy denial and prove no application artifact was written.
Runtime lock/CAS scaffolding is allowed. Terminal replays remove the provider
credential, point HTTP proxies at an unreachable loopback address, temporarily
hide instruction/variable files, and require zero fresh effects and identical
application artifact bytes. This is a replay behavior assertion, not an
operating-system network sandbox.

The interrupted case preserves confirmed non-idempotent effects. An interrupted
in-process fake-provider request may remain uncertain; the runner verifies that
resume refuses it and explicitly records not-applied fixture reconciliation
before continuing. That evidence does not establish exactly-once remote
mutation. Compensation likewise proves a declared best-effort inverse, not a
transactional rollback.

## Real container gate

Example 04's normal mode checks source syntax and the proposed patch. The
additional image gate requires a usable Docker or Podman engine and an already
present Python base image with a repository digest:

```sh
docker pull python:3.12-slim
python3 examples/devops/run.py --agentctl target/debug/agentctl --only 04 --container-build --container-engine docker --keep --report /tmp/agentctl-devops-container.json
```

The runner resolves the base to its immutable digest, builds with networking
disabled, verifies the image's non-root identity, runs it with a read-only root
and no network, captures the image ID and JSON health result, and removes the
fixture image. Substitute `podman` for Docker in both commands if needed. The
base image remains cached. No performance or image-size improvement is claimed.
Without this gate, the report explicitly says container build was not executed.

## Explicit paid OpenAI variants

Only examples 01, 12, 19 and 20 depend on model analysis or model/tool
collaboration. Each has a deterministic fake-provider workflow and a separate
`openai.workflow.yaml`. Ordinary CI executes no paid requests. The crash test's
fake provider is fault injection and has no artificial OpenAI variant.

Paid execution requires the runtime `OPENAI_API_KEY`, available `gpt-5-mini`
access, and an explicit persistent budget shared with every other paid gate:

```sh
python3 examples/devops/run.py --agentctl target/debug/agentctl --mode openai --model gpt-5-mini --live-budget /tmp/agentctl-launch-live-budget.sqlite3 --keep --report /tmp/agentctl-devops-openai.json
```

Use the **same budget file for the entire launch suite and all retries**. A new
file would create a new allowance; it must not be used to bypass an existing
suite limit. [live_budget.py](live_budget.py) exposes `LiveBudget.reserve` and
`reconcile` for other authorized harnesses. Defaults are 100 provider requests,
200,000 total tokens, 1,800 seconds of paid execution and US$25 estimated cost.
SQLite transactions reserve each workflow's request/token/time/cost upper
bounds before dispatch and reconcile durable actual usage afterward. Unknown
usage, unpriced responses and uncertain process deaths retain the entire
reservation. Reasoning tokens are included in output tokens and are not added
twice. Model workflows execute sequentially; the first live failure stops the
paid suite for investigation.

Versioned public-price estimates use gpt-5-mini input at US$0.25 and output at
US$2 per million tokens, recorded as integer micro-US-dollars in the workflow.
These are estimates, not invoices; review [OpenAI pricing](https://openai.com/api/pricing/)
when changing the model or rates. Model overrides with no matching reviewed
pricing contract fail closed. The deterministic remediation workflow uses
explicit synthetic fake-provider prices only to test accounting.

Budget guard regression checks require no credentials:

```sh
python3 -m unittest discover -s examples/devops -p test_live_budget.py -v
```

The [catalog generator](build_catalog.py) regenerates fixtures, workflow
documents, per-example READMEs and the catalog without network access. Edit it
when changing those generated files, then run `python3 examples/devops/build_catalog.py`.
Runtime evidence belongs to the suite report; never replace a failed gate with
a static catalog claim.

[Recorded local validation](validation.json) retains each executed report's source identity, dirty-tree flag, binary hash and results. Refresh this evidence only from actual runner reports:

```sh
python3 examples/devops/record_evidence.py --deterministic /tmp/agentctl-devops.json --container /tmp/agentctl-devops-container.json
```

Add `--live /tmp/agentctl-devops-openai.json` only after executing that paid gate. The recording tool preserves failures and never upgrades local evidence into a final release verdict.
