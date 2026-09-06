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
