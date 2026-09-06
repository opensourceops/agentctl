# PR review follow-up — 6 September 2026

Framework baseline: `bb633fdec35a73f986d7d5d5d74e4f6c72ef6923` (PR7).
Docs baseline: `d4b79f833d0a9d9983a44782d86453cc2e41061e` (PR6).
The dirty original checkout at `/Users/ompragash/agentctl` is preserved; implementation uses isolated checkouts below `.release-evidence/review-20260906`.

Runtime and developer-documentation readiness are separate assessments. The prior passing release suite does not prove that a copied example can be understood, adapted or operated. This follow-up is **in progress**, with no new readiness verdict.

## Findings and decisions

| Finding | Initial verification / disposition |
| --- | --- |
| JSON generated into YAML, JSON-only runner | Reproduced in generator/runner; use pinned `ruamel.yaml==0.19.1`, YAML 1.2, duplicate rejection, readable deterministic output. Rust dependencies remain unchanged. |
| Harness-only journey and parent helper | Reproduced; publish complete packages and explicit setup followed by ordinary agentctl commands. Keep the optional verifier separate. |
| Release-note idempotency | Reproduced: second call fails at Git commit. Move fixture history creation outside analysis; analyze existing repo/range. |
| JUnit errors misclassified | Reproduced with application ValueError. Preserve error/failure/skip and require actual infrastructure evidence. |
| Contradictory advice accepted | Reproduced. Validate bounded structured decisions and evidence, label free prose as advisory. |
| Fixed-token demonstrations | Confirmed in14/17/19/20; preserve bounded contract tests and implement input-driven practical paths. |
| Analysis no-go versus CI gate | Confirmed report-only behavior; keep it and add explicit fail-closed enforcement boundaries. |
| Twenty tutorials absent from site | Confirmed importer includes overview only. Add stable searchable tutorials and complete exact-source assets. |
| Internal release material precedes onboarding | Confirmed. Preserve old routes and reorganize navigation around user tasks. |
| Doctor host dependencies skipped | Confirmed in source; real CLI reproduction and metadata-only fix assigned independently. |

Additional adversarial probes reproduced invalid negative canary counts promoting and a revoked/wrong-component SBOM exception waiving another component. These require semantic fixes, not changed expected results.

## Ownership and validation sequence

Primary owns YAML conversion, common generator/runner/package integration, cases03–07/14–20 integration, ledger, source commits and final gates. Helper reviewer owns named semantic helpers for01/02/08/09/10/12/13 and focused tests. Contract reviewer owns doctor preflight and focused tests. Docs reviewer owns navigation/imports/tutorial prose and onboarding; generators will no longer overwrite long-form tutorials.

1. Preserve parsed YAML values and record pre/post compiled plan identity before behavior edits; source-file byte fingerprints may intentionally change.
2. Integrate named input-driven actions, complete packages, direct command journeys and focused semantic negatives. Execute all twenty direct paths separately from acceptance-runner results.
3. Run changed OpenAI cases only after deterministic semantic checks. Reuse the original absolute paid ledger at `/Users/ompragash/Documents/Codex/2026-09-06/prior-conversation-with-codex-conversation-role/work/evidence/live-budget.sqlite3`; never reset or replace it. Starting charged allowance:57 requests/35,903 tokens/353 seconds/USD2.044363, including one unresolved full reservation. Remaining43 requests/164,097 tokens/1,447 seconds/USD22.955637 estimate.
4. Commit stabilized code, run mandatory release gates, pin paired docs to that exact source, verify actual tutorial assets/browser journeys, and update the same draft PRs. No merge/tag/publication/deployment/production mutation.

Next commands: pinned Python environment setup; `python -m unittest discover -s examples/devops -p test_yaml_authoring.py`; twice `python examples/devops/build_catalog.py`; conversion evidence check; focused helper/doctor tests; all direct example journeys. Record actual outcomes below as work proceeds.

## YAML-only checkpoint

All26 authored YAML documents preserve parsed values exactly after block-YAML conversion. Five focused tests pass for scalar typing, null/missing, multiline newline/Unicode/template strings, YAML1.2 `on`, duplicate rejection and stable serialization. A second catalog generation changed no file. All25 compiled plans were compared at identical paths:24 retain the exact plan digest; case11 intentionally changes because captured `vars/base.yaml` bytes changed. No historical byte fingerprint was rewritten. The acceptance runner's YAML-negative writer uses the same serializer. Tutorial READMEs are now hand-maintained and survive regeneration.

Pinned authoring environment: `python3 -m venv .release-evidence/python`, then `.release-evidence/python/bin/python -m pip install -r examples/devops/requirements.txt`. `AGENTCTL_EXAMPLES_PYTHON` selects that interpreter for credential-free xtask examples/tests; ordinary Rust execution has no Python dependency. CI/RC explicitly install the pinned authoring dependency in a disposable venv. The library is a YAML1.2 parser, not a custom partial YAML implementation ([upstream documentation](https://yaml.dev/doc/ruamel.yaml/)).

YAML runner compatibility used the original bb633fd helper in an isolated copy with the converted workflows and updated YAML reader/writer, before semantic changes. Nineteen cases passed immediately. Case14 was blocked by the local execution sandbox's loopback socket restriction; the same bounded case passed when rerun with the authorized local-server capability. No paid calls. Reports `.release-evidence/yaml-runner-check.json` and `yaml-runner-check-14.json` retain that distinction. The conversion evidence is `.release-evidence/yaml-conversion.json`.

## Semantic and direct-journey checkpoint

YAML implementation commits: `f29b69c` and `e76f194` (the latter fixes serializer line-wrap trailing spaces found by diff checking; six YAML tests now pass). Current semantic work is uncommitted and must not be attributed to these source commits.

Doctor's missing-host-command reproduction returned ready=true/exit0 before the fix; the same plan now returns ready=false/exit6. Metadata-only checks cover explicit executable paths, direct script paths and declared environment references. Bare command lookup and transitive helper dependencies remain explicitly unverified; doctor never dispatches helpers. Seven real CLI tests, four existing source CLI tests and focused Clippy passed.

Named helper operations replace public case-number dispatch. Focused tests cover JUnit application errors, scoped/revoked exceptions, unsupported recommendations, repeatable release-note analysis without Git mutation, explicit no-go gates, input count validation, actual GitHub Actions YAML, pinned Kubernetes schema, supplied vendor patches, observed local service state and captured-state compensation. Packages include all shared helper modules, pinned Python dependencies, setup and source/file digests. Setup configures an absolute selected interpreter while its process grant uses the basename required by the existing policy engine; this is trusted host Python authority, not a per-child-process sandbox.

Direct checks exposed and retained real failures: absolute process grants did not match runtime basename authorization; case06 and CI gates lacked direct data dependencies; case07 Git paths inherited an enclosing checkout; case19 handoff lacked its explicit reviewed-output dependency. These are being fixed in maintained generator/setup sources. Eight analysis paths and three format paths have passed direct check/plan/run/inspect and keyless replay with fixtures removed; final all20 evidence remains pending.

The DSL intentionally forbids a loop around a sub-workflow. Case20 therefore uses the existing action-loop contract for deterministic artifact-validated repair steps. The optional agent stages a target in a bounded proposal loop; an independent validator checks its actual bytes against the source and configured maximum before repair. A model done flag only ends proposal attempts and never proves completion. Nine focused change/remediation tests pass, including 300→180→60→45, invalid staged targets and bounded nonconvergence. Case19 uses a substantive configuration proposal, deterministic reviewer checks and a preserved-fields output copy; rejected proposals cannot reach execution.

Documentation draft checks pass for all20 imported tutorial routes, complete ZIPs, links/anchors, spelling and responsive browser coverage. This is draft evidence against a dirty checkout. Final source pin, direct installed-binary journeys, paired build and release/hosted gates are still required. No paid requests have been made in this follow-up; the original ledger and unresolved reservation remain unchanged.

Next: regenerate coordinated workflows; finish direct19/20 and service/recovery walkthroughs; integrate optional runner using the same package/setup; register all focused tests and inventory changes; commit stabilized code; execute required source/package/hosted gates and changed OpenAI cases using the original ledger; pin and verify paired docs; update both existing draft PRs.


## Integrated cookbook checkpoint

Doctor is committed as `ba4a5a4` after the two YAML commits. All20 primary offline workflows passed the optional suite through the public package/setup path, with zero provider requests in those primary runs. Fake malformed-output and interruption cases remain separately labeled. Seventy-five discovered cookbook regression tests passed, including actual actionlint1.7.7, package completeness/checksums, exact variant inventory, deterministic regeneration and compiled YAML equivalence; two additional runner bookkeeping tests passed. Required full integration/release gates against the final commit remain pending.

Independent direct walkthroughs now cover all20:19 analysis scenarios, five format baselines plus11 variants, nine role/remediation scenarios, and local deployment/recovery/matrix journeys. Case17 restored arbitrary prior version7.8.9 and nested fields byte-for-byte and independently probed them through HTTP. Case15's normal path has no model; its separate manual fault aid pauses30seconds. Interrupted fake requests retain their engine reservation, so the explicitly documented fault recipe budgets two requests/two turns/2,048 tokens for interrupted and resumed attempts. Earlier insufficient-budget failures remain recorded. The deployment mutation remains confirmed once and reused during resume. Case18's direct recorder initially assumed service-major order; the actual compiler sorts matrix axes, and evidence now asserts the documented deterministic order without changing runtime behavior.

The optional Docker04 gate passed the published Podman journey with base `python@sha256:3949e4271b0a3ff82afac7306764c313dcc8edeeb89c0376a3c2ac6007c66b1d`, non-root user65534:65534, read-only/no-network health probe and owned image cleanup. It makes no performance claim. All20 tutorials now have source-derived output excerpts, complete layouts and explicit recovery commands.

Authenticated read-only OpenAI model metadata lookup returned200 for `gpt-5-mini` with normal TLS verification and no environment proxies. No new provider generation has run. Current official [model capabilities](https://developers.openai.com/api/docs/models/gpt-5-mini), [structured-output contracts](https://developers.openai.com/api/docs/guides/structured-outputs) and [standard pricing](https://developers.openai.com/api/docs/pricing) were checked; configured standard estimates remain USD0.25/million input and USD2/million output tokens. These are estimates, not invoiced usage. The first changed-example POST will be recorded by the existing paid ledger. A final review of outstanding runtime reservations is in progress before paid dispatch.

Next commands: finish runner reservation review; `git diff --check`; commit the integrated examples/tests/docs; push the existing framework branch; run `AGENTCTL_EXAMPLES_PYTHON=<venv>/bin/python cargo xtask verify` and mandatory release gates; build/test the installed candidate and execute all20 direct paths with that binary; run only changed01/12/19/20 OpenAI cases against the original ledger; pin and verify paired docs; collect final hosted artifacts and update draft PR descriptions. [The compact cookbook review matrix](COOKBOOK_REVIEW_MATRIX.md) records personas, inputs, behavior and negatives.

Final pre-commit review reproduced an accounting defect in the optional paid runner: its success/failure reconciliation ignored outstanding runtime reservations. Both paths now require complete terminal usage, every reservation counter zero, priced usage and no uncertain provider outcome before releasing shared allowance. Four injected real-SQLite accounting tests pass, alongside seven shared-budget and nineteen wrapper tests. No paid dispatch was needed. The original unresolved reservation remains charged.

Dependency metadata exposed a prerequisite error: pinned rpds-py requires Python3.11+, so tutorials now state3.11+ and hosted CI/RC explicitly select Python3.12 through the official commit-pinned setup action. Dependency versions did not change. The independently reviewed package/setup, schemas, denial, replay and cleanup changes had no other identified blocker; actual final Windows execution remains a required gate.
