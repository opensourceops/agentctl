# Release containers and remediation execution ledger

## Scope and authority

Implement the approved release/container/remediation task from 6 September 2026. Prepare the internal package release 0.4.0, complete native packages and minimal/tooling images, a standalone Trivy-to-two-agent remediation demonstration, and evergreen documentation. Implementation branches and paired draft PRs are authorized. Creating and initializing only `Ompragash/agentctl-remediation-demo` and generating its labeled remediation draft PRs are authorized. No implementation PR merges, release tags, final releases, production registry tags, production deployments or organization security changes.

The original dirty `/Users/ompragash/agentctl` checkout remains untouched. Its branch is `codex/v0.3.0-release`, source `2aeaa88fba71162206b5f08f5bda4f0150247e4f`, with the original CLI/reference/ledger edits and pnpm cache preserved. The clean framework and docs clones were switched to main, pulled with `--ff-only`, then branched for this work:

| Repository | Fresh main | Implementation branch |
| --- | --- | --- |
| opensourceops/agentctl | cddf1cd0634616cca34f6a7a43acf19c500631c7 | codex/release-containers-remediation |
| opensourceops/opensourceops.github.io | d64d62b7e4277d8e7e48203900697a8397298ccb | codex/release-remediation-docs |

Baseline exact-main CI34035910486, container34035910534, security34035910517 and paired Pages34035955453 passed. They establish the starting state only. The new code needs new execution evidence.

## Decisions before implementation

1. Release sequence: an explicit reviewed source (or operator-created exact tag) starts preparation; four native CLI package targets and both Linux image architectures pass full gates. Every binary, checksum, SBOM, provenance record and verified OCI image archive is attached to a draft release before the operator publishes. Publication verifies the immutable release/source/assets and promotes those same image digests without rebuilding. A validation mode uses a disposable registry and never writes production tags. A published release without complete preparation fails with an actionable instruction.
2. Preserve Linux x86_64, macOS ARM64 and Windows x86_64 packages; add native Linux ARM64. Images use native Linux runners for amd64 and arm64. A minimal final/default Containerfile target retains its explicit agentctl entrypoint, CA roots, nonroot user and read-only-root contract. A tooling target adds the explicitly documented shell/Python/Git prerequisites; models get no Docker socket or GitHub publishing credential.
3. Images/archives, not mutable tags, carry identity between gates. OCI provenance-only descriptors are distinguished from the two runnable platforms. Immutable patch tags are checked before writes; stable aliases move only after all required variant/platform gates and promotions succeed. Reruns reconcile existing digests, and all publisher executions serialize.
4. One ecosystem and direct dependency in the initial useful demo: Python urllib3, with supported fixes chosen only from actual upstream advisory/package and Trivy evidence. The before/after scan must use the same captured database snapshot. No lower-count-only success assertion.
5. One agentctl run connects model-backed analysis and implementation through a validated typed plan, followed by trusted outer tests/build/rescan and a separate credential-free eligibility workflow. Eligibility binds source tree, scoped diff, image identities and reports. The publisher operates in a fresh trusted job, verifies those bindings and reconciles a deterministic branch/PR identity. Outer build/publish operations are not misrepresented as agentctl-ledger effects.
6. Candidate mode requires a reviewed framework SHA recorded in the trusted demo source; an arbitrary dispatch input cannot select credential-bearing runtime code. The normal consumer mode uses the configured published image digest only after it exists.
7. Existing `agents.<name>.reasoning.effort: high` and OpenAI Responses tool/schema support already satisfy Astra configuration. No new model syntax or unrestricted coding tool is warranted.
8. Preserve the original paid SQLite ledger and its uncertain reservation. Reserve a task envelope within its remaining allowance, then allocate durable job leases: at most 30 provider requests, 40,000 total tokens, 900 seconds and USD2 estimated spend. Reasoning tokens are included in output tokens once. Every uncertain dispatch remains charged. No provider calls have occurred for this task yet.

## Environment and configuration

Local runtime OPENAI_API_KEY is present; values were not printed. Local GH_TOKEN and DOCKERHUB_TOKEN are absent. Maintainer GitHub authentication belongs to Ompragash and has access to the two framework/docs repositories. The public demo repository was absent and has now been created at https://github.com/Ompragash/agentctl-remediation-demo; it is awaiting the reviewed bootstrap content.

The demo currently has no Actions secrets or variables. The user has been asked to configure OPENAI_API_KEY and repository-scoped GH_TOKEN (Contents and Pull requests read/write), plus AGENTCTL_MODEL=gpt-6-astra. Values will not be copied from another repository or from the local runtime. AGENTCTL_IMAGE remains a post-release digest configuration; candidate mode avoids depending on an unpublished image. Docker Hub username/token configuration belongs to the later operator release handoff.

The existing Podman Linux ARM64 engine works. Its first sandboxed socket probe failed with an access error; the authorized socket probe succeeded (Podman5.8.2). This is not an engine failure. The earlier Mac loader issue remains historical evidence; use functioning isolated Linux/hosted runners where it persists, and do not relabel an unexecuted local gate as passing.

## Work ownership and current checkpoint

- Primary: release metadata, container definitions, release workflows/scripts, source/package/registry verification, GitHub bootstrap/integration and final reports.
- Docs reviewer: maintained public source prose, docs import/navigation/template cleanup and final paired site checks. Historical evidence remains preserved.
- Example reviewer: new complete `examples/devops/21-container-remediation/` package and its deterministic/live/publisher contracts. Existing shared catalog and tests remain primary-owned integration paths.
- Contract reviewer: new release live-budget coordinator/tests, reusing existing ledger semantics; no real reservations or paid calls during unit tests.

New raw evidence belongs under `.release-evidence/release-20260906/`; prior review and launch artifacts remain source-labeled history. Official GitHub immutable-release/event, Docker multi-platform/token, Trivy database, and OpenAI Astra/Responses/pricing documentation has been read. Exact action and image pins are being resolved from their official upstreams.

Remaining gates: implementation; focused contracts; actual application before/after builds and scans; native image execution on both architectures; disposable registry push/manifest/pull; complete platform/package/security gates; bounded actual Astra tool/structured-output demonstration; hosted demo-created draft PR and rerun reconciliation; paired site build/browser/search/download checks. Production publication and its final smoke test remain pending the user's release action.

Next commands: finish public digest/pin capture; implement Containerfile variants/version metadata and release bundle validators; integrate the standalone example and budget lease contract; bootstrap demo main only after credential-free content review; run focused checks before paid or full hosted validation.
