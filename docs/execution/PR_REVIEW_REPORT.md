# PR review follow-up report

Runtime readiness: **launch-ready pre-1.0 candidate for review, unpublished**. Developer-documentation readiness: **ready for review at the paired source, undeployed**. All required gates below passed after the review fixes. This conclusion applies to the stated local/fixture and provider boundaries; it is not stable 1.0 or production-deployment certification.

The [machine-readable evidence](PR_REVIEW_FINAL_EVIDENCE.json) contains exact commands, package/report hashes, source-specific live records and retained failures. The [feature matrix](FEATURE_EVIDENCE.md), [twenty-case review matrix](COOKBOOK_REVIEW_MATRIX.md) and [catalog](../../examples/devops/catalog.json) provide implementation and scenario details.

## Tested identities

- Framework code: `4a22f7f733c5c722263b956b59f36107ec398fc7`; [draft PR #7](https://github.com/opensourceops/agentctl/pull/7).
- Documentation code: `8b3cdda50dea34a7282365f0d09dc7eef219848c`, pinned to that framework source; [draft PR #6](https://github.com/opensourceops/opensourceops.github.io/pull/6).
- Installed Linux ARM64 binary: SHA-256 `55462ef9413222fd1bd0b1755544e675a223a767d183e2f26744fd2433f5d01d`, version 0.3.0, Rust 1.88.0. The version alone is not a candidate identity.
- Changed OpenAI execution remains tied to clean `45d0995345ba048d8a8368466f56c388cc8cb992`. Its installed binary is byte-identical. The exact later diff is reviewed and qualified in the evidence; no new paid execution is claimed for later sources.
- Subsequent execution-ledger/report commits are evidence only. Their SHAs are listed separately in the updated PR descriptions and final delivery; the site retains the tested framework pin.

## Findings and disposition

| Finding | Problem | Disposition and resulting behavior |
| --- | --- | --- |
| 1 | JSON generated into authored YAML | fixed: Pinned YAML 1.2 parser/serializer, readable block YAML, duplicate-key rejection, semantic-equivalence proof and deterministic regeneration cover generator and runner negative variants. |
| 2 | Public journey depends on acceptance harness | fixed: Every case has a complete portable package, pinned declared dependencies, published setup, direct CLI commands and case-specific recovery. Named helper operations leave orchestration visible in YAML; run.py is optional. |
| 3 | Release-note analysis falsely idempotent | fixed: Fixture Git history is prepared separately. Operational analysis consumes an existing repository and explicit ref range; repeated calls preserve Git and produce identical verified output. |
| 4 | Arbitrary JUnit errors classified as infrastructure | fixed: Preserves errors, failures and skips across nested suites and optional attributes; infrastructure classification requires source evidence. Application ValueError is a regression fixture. |
| 5 | Advice verifier accepts contradictory recommendations | fixed with explicit evidence boundary: Controlled decisions must match observations and cited evidence. Contradictory or unsupported actions are rejected; arbitrary model prose remains explicitly advisory. |
| 6 | Fixed-token mechanics presented as reusable operations | fixed with labeled fixture boundaries: Input-driven local deployment, actual HTTP health, captured-state compensation, substantive typed role review and artifact-validated bounded remediation have practical paths. Fake fault aids remain separately labeled. |
| 7 | Successful analysis mistaken for release permission | fixed: Report-only no-go retains exit 0. Separate CI enforcement workflows fail with exit 4 and prove the downstream marker was not created. |
| 8 | Twenty tutorials missing from site/search | fixed: All 20 have stable site routes, search entries, readable/copyable YAML and complete downloads whose manifest/file hashes bind the exact framework source. |
| 9 | Navigation foregrounds internal release records | fixed: Six task-oriented groups prioritize installation, first use, authoring, cookbook and recovery. Advanced history remains reachable at existing routes; workflow API v1 is distinguished from pre-1.0 CLI maturity. |
| 10 | Doctor skips required host process dependencies | reproduced through real CLI and fixed: Metadata-only explicit executable, direct script, cwd, execute-bit and declared environment checks. Missing checked prerequisites or unresolved bare command lookup return not-ready/exit 6. Transitive dependencies and executable loading remain explicitly unverified even when checked metadata is present. |

Additional fixes cover real-format validation, scoped vulnerability exceptions, invalid canary counts, before/after test-byte evidence, retained uncertain paid reservations, Python interpreter discovery, portable Windows ZIP manifests, Windows socket prerequisites, a yanked dependency patch, explicit process-failure phase markers and code-block accessibility. Each arose from a source review or retained failing execution; assertions and production policy defaults remain enabled.

## Twenty independent developer journeys

Every primary workflow passed from its own complete package using the exact installed candidate and declared dependencies, independently of `run.py`. These Linux journeys ran with networking disabled, except loopback inside that isolated network namespace, and no provider credentials. All 20 primary workflows require zero model requests. The separately labeled fake interruption aid and both complete inline onboarding workflows also passed; every replay produced zero fresh effects/provider requests. The first bounded-agent onboarding uses a fake provider explicitly.

| ID | Useful workflow | Direct journey at 4a22f7f | Separate changed OpenAI mode |
| --- | --- | --- | --- |
| 01 | [Failed CI build diagnosis](../../examples/devops/01-ci-diagnosis/README.md) | Passed | Passed at 45d0995 |
| 02 | [JUnit test triage](../../examples/devops/02-junit-triage/README.md) | Passed | Not required |
| 03 | [Pipeline configuration review](../../examples/devops/03-pipeline-review/README.md) | Passed | Not required |
| 04 | [Dockerfile improvement](../../examples/devops/04-dockerfile-review/README.md) | Passed | Not required |
| 05 | [Kubernetes manifest review](../../examples/devops/05-kubernetes-review/README.md) | Passed | Not required |
| 06 | [Governed Terraform plan analysis](../../examples/devops/06-terraform-plan/README.md) | Passed | Not required |
| 07 | [Dependency update assessment](../../examples/devops/07-dependency-update/README.md) | Passed | Not required |
| 08 | [SBOM and vulnerability triage](../../examples/devops/08-sbom-triage/README.md) | Passed | Not required |
| 09 | [Verified release notes](../../examples/devops/09-release-notes/README.md) | Passed | Not required |
| 10 | [Release readiness decision](../../examples/devops/10-release-readiness/README.md) | Passed | Not required |
| 11 | [Configuration drift and variable precedence](../../examples/devops/11-configuration-drift/README.md) | Passed | Not required |
| 12 | [Incident timeline](../../examples/devops/12-incident-timeline/README.md) | Passed | Passed at 45d0995 |
| 13 | [Canary evaluation](../../examples/devops/13-canary-evaluation/README.md) | Passed | Not required |
| 14 | [Approval-gated local deployment](../../examples/devops/14-local-deployment/README.md) | Passed | Not required |
| 15 | [Interrupted deployment recovery](../../examples/devops/15-interrupted-deployment/README.md) | Passed | Not required |
| 16 | [Terminal retry and selective repair](../../examples/devops/16-retry-repair/README.md) | Passed | Not required |
| 17 | [Compensated rollout](../../examples/devops/17-compensated-rollout/README.md) | Passed | Not required |
| 18 | [Parallel service matrix](../../examples/devops/18-parallel-matrix/README.md) | Passed | Not required |
| 19 | [Typed role sub-workflow](../../examples/devops/19-role-subworkflow/README.md) | Passed | Passed at 45d0995 |
| 20 | [Bounded remediation loop](../../examples/devops/20-bounded-remediation/README.md) | Passed | Passed at 45d0995 |

User-supplied input, malformed/contradictory data, no-go gate, repeat execution, threshold, approval and recovery variants are mapped in the review matrix and focused regressions. Required hosted verification separately ran the optional all 20 acceptance runner on Linux, macOS and Windows. Windows CI and RC each report 86 cookbook tests passed and one optional actionlint test skipped because ACTIONLINT_TEST_BINARY was absent; the earlier actual actionlint execution remains separately recorded. The early nine-test native Windows subprocess regression verifies the explicit SYSTEMROOT prerequisite; a real CLI denial proves that removing permission prevents any HTTP request. Arbitrary prior local state is restored and independently probed in compensation; uncertain effects are never blindly retried.

Case 03's actual actionlint 1.7.7 and case 04's actual digest-pinned image build/probe retain their earlier follow-up source/artifact evidence; final direct baselines label these extra gates optional. No image-performance, package-registry-upgrade, Kubernetes admission or Terraform-apply claim is inferred.

## Navigation, onboarding and downloaded artifacts

Six navigation groups prioritize Start, Author, Cookbook, Operate and recover, Reference, and Contribute/architecture while preserving existing routes. All 20 tutorials are imported and searchable. Complete YAML is copy-tested across desktop, tablet and mobile; source-rendered keyboard focus and copy-feedback contrast are covered by the unchanged accessibility assertions. Installation is pinned to the paired Git source, and both first-run pages include complete executable YAML.

The downloaded hosted site artifact `9986762734` has SHA-256 `3ca6d20ecce358c40981be658a254fd9935437ca143c1a6b2c8fa34144af7333` (4,358,390 bytes). It contains 93 HTML pages, the required `.nojekyll` marker, 84 source imports and 20 complete example ZIPs. Metadata and every nested file digest match the source; every nested package manifest is byte-identical to the one used in the installed direct journey. Visible Canonical source boilerplate is absent. The 122 browser passes include all 20 cookbook pages at three viewports; the one skipped generic mobile-search duplicate predates this work, and no new cookbook test is skipped.

## Mandatory gates and release artifacts

| Gate | Result | Coverage |
| --- | --- | --- |
| [credential-free-ci](https://github.com/opensourceops/agentctl/actions/runs/34024602218) | Passed | gates (aarch64-apple-darwin), gates (x86_64-unknown-linux-gnu), gates (x86_64-pc-windows-msvc), production SBOM |
| [rc-release-preparation](https://github.com/opensourceops/agentctl/actions/runs/34024619321) | Passed | RC (aarch64-apple-darwin), RC (x86_64-unknown-linux-gnu), RC (x86_64-pc-windows-msvc) |
| [container-security](https://github.com/opensourceops/agentctl/actions/runs/34024602216) | Passed | container |
| [supply-chain-security](https://github.com/opensourceops/agentctl/actions/runs/34024602237) | Passed | security |
| [Paired documentation](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34024704916) | Passed | Full combined gate; 122 browser passes, one existing duplicate skip; deployment skipped |

Commands include `cargo xtask verify`, `docs-verify`, `acceptance`, `completeness`, `package` and `acceptance-container`; verification includes generated/schema/CLI/example freshness, the complete Rust/Python suites, exact YAML inventory, source installation, secret/dependency checks and production boundaries. Artifact-store, migration and protocol-resilience test filters are included in the full Rust suite. Hosted security additionally checks full history, action pins and image vulnerabilities. The paired site executes `AGENTCTL_REPO=<exact checkout> pnpm verify:agentctl`, including links, anchors, Mermaid, search, writing and browser checks.

All eight downloaded framework artifact ZIPs match their GitHub Actions digests; every packaged binary matches its inner SHA256SUMS. The download records bind the production and image CycloneDX SBOMs to their exact-source jobs. The image SBOM records the local image ID; that job builds, tests and scans the same local tag. No independent console-log image-ID match or published registry image is claimed. Package README/LICENSE files match the source, with an explicit CRLF checkout conversion on Windows. Independently built Linux/macOS binaries match across CI/RC; Windows binaries differ and retain separate verified digests. No bit-for-bit build reproducibility guarantee is claimed.

| Artifact | Actions ID | Downloaded ZIP SHA256 |
| --- | --- | --- |
| agentctl-production-sbom-cyclonedx | 9986932107 | `076e1c35449fd0ad5e85c82a44d432cf64cf81842b79ffdebe32a43e171f4ae8` |
| agentctl-x86_64-pc-windows-msvc | 9986927030 | `0be47b7a8099429006fcf98bbcbc8e667e0dcd4349b3f19d64c33ff54712f4bc` |
| agentctl-aarch64-apple-darwin | 9986812868 | `acaf93f4cca1d7afaa0235c2e23d1d889c4c87a9e2a18b89ff3e6453e1b66d2c` |
| agentctl-x86_64-unknown-linux-gnu | 9986808934 | `7bfc005f1a4bdbc241a8661bf1d52ac4d58c20c74d81d55e56f572b123bd0aab` |
| agentctl-rc-x86_64-pc-windows-msvc | 9986946853 | `83824bd16a42e32d8e6a589a61c16d1dba6c76322e72d9a5fb2d2c2c485b2fd9` |
| agentctl-rc-x86_64-unknown-linux-gnu | 9986810807 | `354027c16778f76bedc2163b038d7c56b33a4197a51d433bb10f37d10849de6b` |
| agentctl-rc-aarch64-apple-darwin | 9986796195 | `b0431d6eda582545fe8de25f31ed4f875c940e94546ba64b816e58a8b75984a4` |
| agentctl-image-sbom-cyclonedx | 9986718202 | `8d729d1b10bb186f72c0f5542f3f6f1b06d6a117500a12a5205b38f67961a6d5` |

Exact Git installation used:

```sh
cargo install --locked --git https://github.com/opensourceops/agentctl --rev 4a22f7f733c5c722263b956b59f36107ec398fc7 agentctl-cli --root /work/installed-4a22f7f
```

The `/work` path is the retained isolated validation environment. Installation took 5.821 seconds using existing verified build caches; the 20 direct journeys plus fake interruption supplement took 112.142 seconds. These are observations, not performance-improvement claims.

## Compatibility and paid accounting

Existing exclusive inline `instructions` versus `instructionsFile` behavior is preserved. Variable precedence remains low-to-high: workflow files → workflow inline → agent files → agent inline → task files → task inline → invocation files → invocation inline. Later repeated files/flags replace whole top-level keys, including objects, arrays, scalars and null. Typed inputs remain separate; ordinary variables import no environment or secret authority implicitly.

Changed live cases 01/12/19/20 passed with `gpt-5-mini`: 9 requests, 4,956 input plus 1,067 output tokens (6,023 total), 27 charged wall seconds and USD 0.003377 estimated cost. Structured decisions, evidence relationships, real staged configuration, typed role handoffs, deterministic remediation validation, denials and keyless replay were asserted. Free-form advice remains advisory.

The original shared ledger is retained. Known reconciled totals are 54 requests/16,390 tokens/140 seconds/USD 0.047740 estimate. One earlier uncertain reservation remains charged at 12 requests/25,536 tokens/240 seconds/USD 2.000000. Charged totals are 66 requests/41,926 tokens/380 seconds/USD 2.047740; remaining allowance is 34 requests/158,074 tokens/1,420 seconds/USD 22.952260. Reserved counters are not actual usage; costs are estimates, not invoices. Earlier `gpt-5.6-sol` and `gpt-6-astra` successes and failed whole invocations retain their original source labels. No unchanged successful paid case was rerun merely to obtain a single green inventory command.

## Remaining limits and retained failures

There is no unresolved required release gate for the tested sources. This Mac still has a recorded loader problem affecting new Mach-O and some shell launches: interrupted local full-Cargo and combined-site commands remain non-passing. Installation/direct execution used functioning isolated Linux, and mandatory complete verification used actual hosted Linux/macOS/Windows. The site’s unchanged underlying 14 commands also passed locally through pinned Node entrypoints. No host security, daemon, TLS, production or organization setting was changed.

Earlier inventory, phase-timing, interpreter, Windows path, Windows socket, dependency, accessibility and environment failures remain in the execution ledger and hashed failure index. A cancelled run is never treated as a pass. The final corrections preserve source-specific successful prefixes and qualify their applicability.

- Local service and cloud-plan examples are disposable fixtures; no production deployment or Terraform apply is claimed.
- Trusted host Python is not an OS sandbox for its child processes or file writes.
- Schema validation does not establish Kubernetes admission or live-cluster conformance.
- The supplied vendored-code patch journey is not a package-registry upgrade implementation.
- Compensation is best-effort inverse execution; uncertain remote effects require evidence and reconciliation.
- Fresh model output is nondeterministic; deterministic control does not imply exactly-once remote mutation.
- Live services tested are OpenAI and isolated local/OCI fixtures; no other provider, real MCP/A2A peer, cloud account or production infrastructure live coverage is inferred.
- No performance or cost savings are claimed. Recorded duration and usage are observations, not comparative benchmarks.

Merge, release tags, package publication and public deployment remain separate review actions. Both existing PRs stay draft.
