# Remediate a scanned container dependency

For a platform engineer who needs a reviewable dependency fix backed by an actual application build and vulnerability rescan.

This standalone application deliberately pins vulnerable `urllib3==2.6.2`. A trusted CI runner builds and tests its image, captures a real Trivy report and one immutable database snapshot, and invokes two bounded Astra roles through an agentctl tooling container. The roles propose and stage only the reviewed `urllib3==2.7.0` manifest and wheel hash. CI then tests, rebuilds and rescans the exact changed tree. A separate credential-free agentctl workflow decides publication eligibility; a fresh publisher job creates or reuses a draft PR in [Ompragash/agentctl-remediation-demo](https://github.com/Ompragash/agentctl-remediation-demo).

The sample application fetches a bounded response preview with redirects disabled. Do not install its deliberately vulnerable dependencies into your workstation or tooling environment.

## What owns each operation

| Stage | Authority and observable result |
| --- | --- |
| Trusted prepare | Exact Git source archive, pinned Python base, application tests, real Trivy JSON, database and metadata SHA256 |
| Analyzer A | No tools; strict plan naming captured advisories, evidence pointers, source/report identity, allowed files and fixed version |
| Deterministic validation | PEP440 version checks, direct dependency mapping, complete advisory coverage and exact reviewed wheel inventory |
| Implementer B | Two `builtin.workspace.write` tools; each accepts one exact staging path and exact reviewed content |
| Deterministic patch validation | Actual file bytes, original manifest hashes, allowed file inventory and stable source/patch fingerprint |
| Trusted outer validation | Exact Git tree, application tests, actual image rebuild and rescan using the original database bytes |
| Credential-free eligibility | All targeted findings absent, approved dependency installed, no new HIGH/CRITICAL finding, consistent source/patch/image/report/database identities |
| Separate publisher | Only the configured repository/default branch/prefix; exact validated tree; draft PR; read-only reconciliation after uncertain push/create |

A successful eligible result may retain unrelated baseline findings. Every residual finding remains in `publication.json`; none is described as fixed. Unsupported ecosystems, non-direct dependencies, unfixed advisories and fixes outside the reviewed release require manual review. A completely clean report produces `no_change`, zero model calls and no PR. A report containing only unsupported findings produces `manual` and no PR.

## Package contents

- [Application](app.py), [Dockerfile](Dockerfile), [direct manifest](requirements.in), [hash lock](requirements.lock), [application tests](tests/test_app.py).
- [Two-role workflow](agentctl/remediate.yaml), [eligibility workflow](agentctl/eligibility.yaml), [analyzer instructions](agentctl/instructions/analyze.md), [implementer instructions](agentctl/instructions/implement.md).
- [Adapter and validator](remediation/adapter.py), [reviewed dependency/scan contract](remediation/contract.json), [typed schemas](remediation/schemas.py).
- [Trusted runner](remediation/runner.py), [candidate/published tooling selection](remediation/bootstrap.py), [publisher](remediation/publisher.py), [GitHub Actions workflow](.github/workflows/remediation.yml).
- [Synthetic CLI contract runner](remediation/contract_check.py), [adapter tests](tests/test_adapter.py), [package and publication tests](tests/test_contracts.py).

No sibling framework checkout or twenty-example cookbook runner is required.

## Install and run the credential-free checks

Use a Linux Docker runner, or macOS with a working Docker/Podman Linux engine. The host helper environment needs Python 3.12+ and Git. Actual application builds and scans require registry/package downloads. The pinned application and scanner image indexes support `linux/amd64` and `linux/arm64`. Windows hosts and Harness are not claimed as executed integrations for this package.

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r remediation/requirements-tools.txt
.venv/bin/python -m unittest discover -s tests -p 'test_adapter.py' -v
.venv/bin/python -m unittest discover -s tests -p 'test_contract*.py' -v
```

These tests use explicitly synthetic scanner/model data. They verify parsing and policy boundaries without installing the vulnerable application dependency or contacting a provider. The application itself is tested inside its built image by the trusted runner.

For an already exported package, verify its source manifest before the first remediation:

```sh
.venv/bin/python remediation/export.py --verify .
```

## Configure the demonstration repository

Under the demonstration repository's **Settings → Secrets and variables → Actions**, configure:

| Name | Kind | Scope |
| --- | --- | --- |
| `OPENAI_API_KEY` | Secret | Provider calls in the `remediate` job only |
| `GH_TOKEN` | Secret | Fine-grained token for Contents read/write and Pull requests read/write on this demo repository only; supplied only in the final publisher step |
| `AGENTCTL_MODEL` | Variable | Exactly `gpt-6-astra`; mapped into each actual agent's model setting, with `reasoning.effort: high` configured separately |
| `AGENTCTL_IMAGE` | Variable | Exact reviewed `docker.io/opensourceops/agentctl@sha256:…` tooling image after publication |

The demo does not need Docker Hub publishing credentials. Repository secrets are independent of the framework's secrets. Set an appropriate token expiry and satisfy any applicable organization approval. Do not give the publisher token framework or documentation write access. A PAT/App token may trigger downstream PR checks; the built-in GitHub token has different workflow-trigger rules. This workflow keeps its ordinary GitHub token read-only and uses the requested scoped publisher secret.

The workflow only permits paid remediation through an explicit `workflow_dispatch` on this repository's `main`. Pull requests run credential-free contracts. It does not use `pull_request_target`.

## Candidate and published tooling images

The exporter creates `remediation/bootstrap.json` with an explicit reviewed framework SHA allowlist. Candidate mode fetches exactly that public source commit, verifies it and builds the `tooling` target from its `Containerfile`. It records the framework revision, image ID and tooling labels. It does not use an unreviewed arbitrary branch or an unavailable production tag.

For a local candidate preflight, use the one SHA recorded in your exported package:

```sh
export AGENTCTL_MODEL=gpt-6-astra
FRAMEWORK_SHA=$(.venv/bin/python -c 'import json; print(json.load(open("remediation/bootstrap.json"))["allowedFrameworkShas"][0])')
.venv/bin/python remediation/bootstrap.py --mode candidate --reference "$FRAMEWORK_SHA" --output state/tooling.json
TOOLING_IMAGE=$(.venv/bin/python -c 'import json; print(json.load(open("state/tooling.json"))["image"])')
.venv/bin/python remediation/contract_check.py --tooling-image "$TOOLING_IMAGE" --out state/contracts
```

The synthetic CLI contract executes actual process adapters and both actual file-write tools in an OCI container, plus denial and keyless/networkless replay. The existing fake provider emits one tool call per scripted agent, so this test explicitly splits implementer B's two writes into two scripted stages. The live workflow keeps one implementer with both scoped tools. A synthetic test pass is not live Trivy/Astra evidence.

After the reviewed tooling image is published, run the same contract against its configured digest:

```sh
.venv/bin/python remediation/bootstrap.py --mode published --reference "$AGENTCTL_IMAGE" --output state/published-tooling.json
.venv/bin/python remediation/contract_check.py --tooling-image "$AGENTCTL_IMAGE" --out state/published-contracts
```

The final production-image smoke remains pending until that image exists; candidate-image evidence cannot establish publication.

## Execute the actual CI journey

The trusted coordinator preallocates a source/run/job-bound, nonsecret budget lease before dispatch. It reserves from the existing suite ledger rather than creating an independent paid allowance. The live job receives only its slice. In the reviewed framework checkout, use `python3 scripts/release_live_budget.py --help` for `init`, `lease`, `receipt` and `reconcile`; a lease can bind the next workflow run number before dispatch. The workflow name, source SHA, attempt and job ID `remediate` must match. A concurrent dispatch fails closed.

Dispatch **Bounded container remediation** with:

- `image_mode: candidate` and the exact allowlisted `framework_sha` before publication, or `image_mode: published` after configuring the exact digest.
- `budget_lease`: the coordinator's nonsecret JSON lease bound to this invocation.

The default operation is `remediate`. The authored runtime ceilings are four provider requests, 20,000 total input-plus-output tokens, 600 seconds and US$1 estimated cost. The analyzer has one turn; the implementer has at most three turns and two tool calls. Both have 2,048 maximum output tokens. The coordinator also reserves aggregate allowance before dispatch, reconciles actual durable usage and retains uncertain reservations. Runtime ceilings must fit the assigned lease. The dated pricing estimate is recorded in the workflow, not presented as a guaranteed invoice.

CI executes these trusted stages from the demo checkout:

```sh
.venv/bin/python remediation/runner.py prepare --out "$PWD/state/remediation" --tooling-image "$TOOLING_IMAGE"
.venv/bin/python remediation/runner.py remediate --out "$PWD/state/remediation" --lease "$LEASE_FILE"
.venv/bin/python remediation/runner.py validate --out "$PWD/state/remediation"
.venv/bin/python remediation/runner.py bundle --out "$PWD/state/remediation"
```

The leased paid stage requires the actual GitHub job identity and is intended for the declared CI job. Do not fabricate GitHub identity to reuse a lease locally. `prepare` and the synthetic contract are credential-free local Docker journeys. Pass `--engine /absolute/path/to/podman` on each local runner command when using Podman. To reuse an already captured Trivy database for local validation, pass `prepare --database-snapshot /absolute/path/to/db`; both `trivy.db` and `metadata.json` are mandatory.

The runner uses descriptive local `agentctl-remediation-demo:before-<source-sha>` and `after-<validated-tree>` tags, while recording actual image IDs. It captures Docker-format image archives for Trivy. Scanner exit zero only means the scanner completed; vulnerabilities are passed to the validator rather than suppressed with `--ignore-unfixed`. Both scans run offline with the exact database directory mounted read-only, and hashes are checked before and after each scan.

## Container mounts and grants

The agent container runs with the host UID/GID, a read-only root filesystem, dropped capabilities, no privilege escalation and bounded memory/process resources. Its working directory is `/workspace`:

| Path | Access |
| --- | --- |
| `/workspace/source` | Read-only complete committed source archive and trusted helpers |
| `/workspace/inputs` | Read-only captured reports, metadata and nonsecret lease |
| `/workspace/remediate.yaml`, `/workspace/eligibility.yaml` | Read-only configured workflow copies with workflow-relative instruction paths |
| `/workspace/patch` | Writable staging directory; tool schemas only permit the two fixed dependency files and exact content |
| `/workspace/state` | Writable SQLite/CAS and deterministic evidence records |
| `/ci-budget` | Separate persistent job ledger/lease claims, controlled by the trusted coordinator |

Only the live container receives `OPENAI_API_KEY` and the nonsecret job binding. The model receives no Docker socket, GitHub token, Git tool, shell tool or process tool. Trusted fixed process adapters run `/usr/bin/python3`; their input schemas do not accept executable paths or shell commands. Online agentctl traffic is allowed only to `api.openai.com`; no TLS bypass or inherited proxy grant is configured. Local corporate CA/proxy requirements need a reviewed explicit deployment configuration.

## Inspect evidence and recover

`state/remediation/workspace/state/run.sqlite3` records the A+B run and `eligibility.sqlite3` records the independent post-build decision. `runs.json` records both run IDs and their replay IDs. Each successful run is inspected and replayed without a key and with container networking disabled. Replay asserts zero fresh effects/provider requests and unchanged patch bytes. Builds, scans, Git pushes and PR creation are trusted outer effects, outside these agentctl replay scopes.

The workflow retains command/exit-code logs, before/after reports, database snapshot, source/patch identities, SQLite/CAS, configured YAML, tooling identity, budget receipt and the publication bundle. A missing or failed prerequisite is a failing result, never a scan or release pass.

- **Malformed report/plan or forbidden path:** no eligible publication. Read the source diagnostics and adapter stderr; repair the input or reviewed contract, then validate a new source commit.
- **Missing credential, model access or budget exhaustion:** stop the live stage. Keep its execution ledger and receipt diagnostics. Never reset an uncertain reservation to repeat a paid request.
- **Interrupted agent run:** inspect the retained effect ledger first. A durably paused run may be resumed within the same job/lease using the coordinator's `execute … agentctl resume RUN_ID --db …` command. Unknown dispatch outcomes require reconciliation; a fresh runner is a new invocation, not automatic resume.
- **Failed application test/build/rescan:** no publisher job runs. The validated staged diff and failure logs remain inspectable. A successful scan of the unchanged image or a lower aggregate vulnerability count cannot qualify a patch.
- **Moved default branch:** the publisher refuses stale evidence. Analyze, build and rescan the new source instead of silently rebasing.
- **Uncertain push or PR creation:** read back the stable source/patch-derived branch. The publisher reconciles a lost acknowledgement and never blindly repeats a PR POST. Repeating publication reuses the existing matching draft; a matching closed PR does not cause a duplicate.

To exercise no-paid reconciliation, dispatch the same workflow with `operation: reconcile`, `prior_run_id` and `prior_run_attempt` from the retained validated execution. Leave `budget_lease` empty. This path verifies the original repository, exact source SHA, workflow path, attempt, successful `remediate` job and unexpired publication artifact before downloading it. The overall prior workflow may have failed at an uncertain publisher boundary; the validation job itself must have succeeded. Artifact digest mismatch is fatal. It then recomputes eligibility and the Git tree and invokes only the trusted publisher. It does not inject `OPENAI_API_KEY`, invoke a model, rebuild or rescan. The read-only built-in token has Actions read permission only to retrieve this same-repository evidence; the scoped `GH_TOKEN` remains confined to publication. A moved default branch still blocks publication.

Keep the full same-job workspace and `/ci-budget` directory to recover within that job. A new GitHub attempt needs a new lease or reuse of the prior completed publication artifact. Downloaded evidence is useful for audit and replay; it does not by itself authorize fresh paid work. Do not use GitHub's rerun button with an already consumed lease.

## Cleanup and supported limits

No application image is pushed to a registry, no release is created and no PR is merged. Remove only the local image IDs/tags recorded by this run after reviewing the artifacts. For the runner's retained candidate checkout, run `git worktree remove --force "$PWD/state/remediation/candidate"` before removing the run directory. Keep failed-run evidence until reconciliation is complete. The source/example contract deliberately supports one Python direct dependency and one reviewed upgrade; it is not a general dependency resolver or unrestricted coding agent.

## Synchronize the reusable package and demo

From the framework's clean reviewed commit:

```sh
python3 examples/devops/21-container-remediation/remediation/export.py --output /absolute/new-demo-directory --framework-sha "$FRAMEWORK_SHA" --archive /absolute/new-demo.zip
```

The destination and optional archive must not exist. `package-manifest.json` records the exact framework SHA, package path and every exported file digest, including the candidate allowlist. ZIP names use POSIX paths under `21-container-remediation/` and deterministic timestamps. The exporter refuses dirty package bytes or a different HEAD by default; `--allow-dirty-preview` creates an explicitly labeled local preview that ordinary verification and live CI reject.

Initial demo bootstrap is a reviewed export commit. Subsequent remediation PRs intentionally change only the two dependency files; do not regenerate unrelated helpers or the package manifest inside a model patch. To upgrade the reusable example later, review a fresh export and its content differences as a separate maintainer change.

The live report and generated draft PR are recorded by the execution evidence for the actual source/run. Synthetic fixtures use `CVE-FIXTURE-*` identifiers and must never be presented as live vulnerability or model coverage.
