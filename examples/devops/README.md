# DevOps and CI/CD workflows

Choose a complete local workflow for a concrete CI, platform, SRE, release or security problem. The twenty tutorials below lead with editable YAML, input files and direct `agentctl` commands. The CLI is pre-1.0; `agentctl.dev/v1` names its workflow document format.

## Get a complete package

Install the [matching candidate binary](../../docs/guides/INSTALLATION.md). On the documentation site, each tutorial offers a ZIP containing its workflow, fixtures, instructions, schemas and reviewed helper. From a source checkout, the equivalent packaging command is:

```sh
python3 examples/devops/package.py --example 02 --output ./junit-example
cd junit-example
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python setup.py
agentctl check local.workflow.yaml --workspace .
agentctl plan local.workflow.yaml --workspace .
agentctl run local.workflow.yaml --workspace . --db state.db --output json
```

Use `.venv\Scripts\python.exe` on Windows. Python 3.11 or newer is required; the requirements file pins the YAML authoring dependency and any case-specific validator. Setup configures the selected interpreter and support tools before creating `local.workflow.yaml`. It does not grant authority through user inputs or vars. Inspect the generated local policy, then use the case's direct commands for approvals, recovery or local service operation.

Read [variable precedence](../../docs/VARIABLES.md) and [configuration drift](11-configuration-drift/README.md) when adapting inputs. Captured source files belong to one run: edit for a fresh invocation or a reviewed repair, and use recorded replay to reconstruct history.

## Choose a tutorial

| # | Workflow | Observable result |
|---|---|---|
| 01 | [CI diagnosis](01-ci-diagnosis/README.md) | Parsed failure and structured advice with verified log citations |
| 02 | [JUnit triage](02-junit-triage/README.md) | Real XML parsing, preserved JUnit error/failure kinds and evidence-based classifications |
| 03 | [Pipeline review](03-pipeline-review/README.md) | Permission/timeout violations and a checked, applied Git patch |
| 04 | [Dockerfile review](04-dockerfile-review/README.md) | Narrow COPY, non-root user, code checks and separate real image gate |
| 05 | [Kubernetes review](05-kubernetes-review/README.md) | Local manifest review with explicitly pinned schema coverage |
| 06 | [Terraform plan](06-terraform-plan/README.md) | Plan rules, durable approvals, isolated local-file mutation |
| 07 | [Dependency update](07-dependency-update/README.md) | Vendored patch plus failing-before/passing-after behavioral tests |
| 08 | [SBOM triage](08-sbom-triage/README.md) | Component joins, severity rules and expiring exceptions |
| 09 | [Release notes](09-release-notes/README.md) | Existing repository range and verified changed-file citations |
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

## Authority and evidence

The normal paths work without credentials. Four tutorials also have separately labeled OpenAI workflows; the scripted fake provider establishes deterministic protocol behavior, not live model quality. Other cases perform useful deterministic parsing, patching, validation or routing without artificial model calls.

Offline practical workflows accept supplied local files. Recovery and local-service tutorials are contract demonstrations with explicit disposable fixtures. None applies infrastructure or deploys production services. Report generation can intentionally succeed with `no-go`; tutorials 08 and 10 provide separate CI enforcement paths that fail nonzero and block downstream effects.

The Python process grant trusts the reviewed helper and its documented child processes. It is **not an operating-system sandbox** enforcing a separate policy around every Python file access or Git invocation. Setup records support-tool paths and fingerprints; ordinary variables cannot grant new process or filesystem authority. Use trusted local or disposable runners, protect input data and SQLite history, and review complete packages before execution.

Approval commands record the local actor and reason. Local database ownership is the current authority boundary; the fixture does not simulate an actor-role authorization system. Review each pending operation and its content digest before approving it.

## Contributor verification

The optional acceptance runner packages and prepares the same published workflows, then adds separate denial cases, fault injection, replay assertions and aggregate evidence. A new user does not need the runner to operate an example.

From the framework checkout:

```sh
cargo build -p agentctl-cli --locked
python3 -m pip install -r examples/devops/requirements.txt
python3 examples/devops/run.py --agentctl target/debug/agentctl --report /tmp/agentctl-devops.json
```

Use `--only 01` to select a case, repeat it to select several, and use `--keep` to retain workspaces. The discoverable suite alias is `cargo xtask devops-examples`. Reports retain source and binary identity, commands, exit codes, semantic assertions and artifact hashes. Passing static `check` alone does not establish execution evidence. The [machine-readable catalog](catalog.json) and [recorded validation](validation.json) preserve exact inventory and source-labeled results.

Hand-maintained tutorial prose is separate from the deterministic [catalog generator](build_catalog.py). Regeneration updates workflows, input fixtures and machine-readable inventory; it must not overwrite these READMEs. Run it after authored generator changes and verify a second pass produces no diff.

### Optional container verification

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

### Paid acceptance verification

Examples 01, 12, 19 and 20 provide optional model analysis or model/tool
collaboration through a separate `openai.workflow.yaml`. The primary 01 and 12
workflows are deterministic without a provider; their explicit
`contract.workflow.yaml` files retain scripted fake-agent regression cases.
All four primary workflows work without a provider. Role and remediation examples also keep bounded fake-agent paths as labeled contracts. Ordinary CI executes no paid requests. The crash test's
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
