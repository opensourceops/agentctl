# Release and remediation checkpoint — 8 September 2026

This checkpoint supersedes the 7 September handoff's current-state claims. Earlier failures and source-specific evidence remain in [the execution ledger](RELEASE_REMEDIATION_20260906.md). The complete requested outcome is **not ready**: full live remediation, its model-generated draft fix PR, provider-free reconciliation and the published-image consumer run remain unproven.

## Source and completed changes

Framework [PR10](https://github.com/opensourceops/agentctl/pull/10) is merged. Code under test is `0d4542cccdbd22cb96e3dec25cdffb66d0938ced`; merge commit is `57960bb123b3ffe7a1e5587e2d34e027b4ed18ef`. This handoff and its accompanying ledger entry are evidence-only additions after that tested source, not new release code.

Live testing exposed several defects now fixed: exact multiline tool inputs retain anchored patterns and add exact lengths; preflight exercises the real nested input schemas through a pure echo; absent or invalid required OpenAI usage counters fail with an uncertain effect and retained budget reservation; incomplete Responses preserve the actual normalized reason and known usage without dispatching partial tools. Existing serialized finish reasons, completed-response parsing, write authority and recovery boundaries remain compatible. Earlier incomplete responses cannot be retroactively classified because the old adapter discarded their raw reason.

Local validation passed 35 provider tests, recorded-enum compatibility, six runtime completion/terminal scenarios, two accounting/recovery regressions, 26 shared-budget coordinator tests, 17 example contracts, 13 adapter tests, and workspace/all-target Clippy with warnings denied. The actual OCI fixture passed 66 CLI invocations, including 19 denied malformed writes with zero effects, exact echo/output validation, zero-effect replay and selective repair. Final-source native tooling contracts separately exercise those behaviors.

## Exact-source gates

| Gate | Evidence |
| --- | --- |
| Linux/macOS/Windows CI and production SBOM | [34164610986](https://github.com/opensourceops/agentctl/actions/runs/34164610986), passed |
| Supply-chain/security | [34164610859](https://github.com/opensourceops/agentctl/actions/runs/34164610859), passed |
| Container/security | [34164610910](https://github.com/opensourceops/agentctl/actions/runs/34164610910), passed |
| Four native packages, four native images and signed bundle preparation | [34164848388](https://github.com/opensourceops/agentctl/actions/runs/34164848388), passed; draft attachment skipped |
| Paired documentation | [34165365905](https://github.com/opensourceops/opensourceops.github.io/actions/runs/34165365905), passed; deployment skipped |
| Demo maintenance contracts | [34164752648](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34164752648), passed; paid jobs skipped |

Local raw evidence is under `.release-evidence/release-20260906/` in the original checkout. All eight final native artifact ZIPs have been downloaded and independently checked. `native-release-34164848388/platform-details.json` has SHA256 `8006f1d7385313411f9b4df5234f0e8eebf6dfb73f9c48579e116fdd7431fb02`: native binary formats/architectures/modes, source and SBOM bindings, image configs, and both tooling platforms' denial/preflight/replay records passed. Independent full-bundle verification also passed: all ten Actions ZIPs match their recorded hashes, all 35 checksum-listed files and both complete OCI indexes verify, and a separate attestation check binds the exact source and signer workflow with self-hosted runners denied. The disposable registry created six expected tags, reused all six on rerun, and pulled all four expected platform configs.


| Final retained release evidence | SHA256 |
| --- | --- |
| Full bundle ZIP (254,215,770 bytes) | `29b0afe19050ea746ce570211c52604fc404d52bd33f1eb75e92cad81291b445` |
| Signed release manifest | `0c4ecd8361eb2f7acfd474fc8db643c973850a7cef657af9f0d203ad239c5c02` |
| Sigstore bundle | `aaef830540d6fa14315d3d9d93a5d9cb3609e87fd85c2085ff560d8ee71f4773` |
| Independent `validation.json` | `01e6993b3007ffd6b1da875cc03d4572f81641caef271d05ef8cb6f993f0143e` |
| Native package/archive/binary/SBOM observations | `6877ca212f3c45ed2847749e34ba1445e6d1d03694d7858d8bb1b70663e7b158` |

The final minimal multi-platform index is `sha256:ab71bb86a008b82c8ccabcd6948e3745a11b21727c310d15a05afe4cddb900df`; tooling is `sha256:e16aa7fac1824665d5b54a2a795a308a9e676805a8ef3eed76c7047fd5dc906c`. These were tested only in the disposable registry; they have not been published to Docker Hub.

Documentation [PR7](https://github.com/opensourceops/opensourceops.github.io/pull/7) remains draft and unmerged. Tested docs source `60d4bfacc31a582acff7b59dfd41d2ae73a29833` pins framework `0d4542c`; head `583b4062e4ef181ba5cb3cfc8fde70ff8acc6e32` adds eight evidence-only ledger lines. Local `AGENTCTL_REPO` paired verification and hosted validation passed 140 browser checks, with one unchanged generic mobile-search skip and dedicated mobile cases passing. The actual artifact has 94 HTML pages, `.nojekyll`, all 20 original packages and the independent remediation package; manifests and all nested file hashes match source and clean export. Visible canonical boilerplate is absent.

The downloaded docs ZIP SHA256 is `ab6db1a1a59cc275fc060d2c2b05cde6026fdcae67dba75091a40eb2c17c541d` (4,371,118 bytes). Consolidated `docs/final-docs-report-0d4542c.json` SHA256 is `e3ca7edd40038a2d6b80fedbf08683051dd03e8f31dbde92712f042d34d03c0e`. Hosted artifact retention currently ends 21 September 2026.

## Live evidence and retained uncertainty

The configured model remains `gpt-6-astra` with high reasoning. Ten provider calls were observed across this task. Nine reconciled calls report 5,792 tokens and 73 seconds; the known estimate is USD0.159235 including a separately recorded USD0.002970 historical cache-write correction. These are estimates, not invoice claims.

Preflight15 [34161943315](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34161943315) has unknown actual usage. Its entire lease remains reserved: 3 requests, 8,000 tokens, 120 seconds and USD0.20. No zero-cost reconciliation is claimed. Preflight18 [34163342194](https://github.com/Ompragash/agentctl-remediation-demo/actions/runs/34163342194) had explicit numeric zero required usage counters and was reconciled as one reported-zero-token request, four seconds, with its original receipt unchanged. Neither run proves output-cap exhaustion or content filtering.

The task ledger therefore holds 12 charged/reserved requests, 13,792 tokens, 193 seconds and USD0.356265, plus the separately retained USD0.002970 pricing correction. Its envelope remains 30 requests / 40,000 tokens / 900 seconds / USD2 estimated within the original shared allowance. Original historical charges and uncertainties remain untouched; no next lease has been issued. Do not close the envelope while preflight15 is unresolved.

The earlier simple echo preflight passed, but both full remediation attempts and all stronger multiline preflights failed. No model-generated fix PR, successful full live remediation, provider-free PR reconciliation, or full published-image consumer smoke exists. Maintenance PRs are not remediation output. `OPENAI_INCOMPLETE_REPRO.md` in local evidence records public-fixture response IDs for diagnosis without another paid request.

## Approval-dependent continuation

Automatic approval review rejected three actions. No rejected command was executed. Explicit questions remain pending:

1. Merge demo maintenance [PR7](https://github.com/Ompragash/agentctl-remediation-demo/pull/7). Head `8c54c077f634a73f9ba04c4fff9650dc49d63bf4` changes only the framework pin and package manifest; contracts pass. Draft publication was subsequently accepted as the authorized alternative. Making it non-draft or merging remains pending explicit approval. Trusted main is still `bdba15bfdeac13b0f4a0b84a6dbcb7a2786f7a59`.
2. Merge docs PR7 and allow its normal public GitHub Pages deployment. The merge was rejected because of the deployment side effect; validation does not grant deployment approval.
3. Replace the task-created unpublished v0.4.0 tag and subsequently publish the verified release and Docker Hub images after live gates pass. The earlier pending question named superseded `ed60436`; the final reviewed source is now `0d4542cccdbd22cb96e3dec25cdffb66d0938ced` and must be explicit when resolving approval.

The old annotated v0.4.0 tag object remains `23bb0c6de265273f4c7a3ebbba72d0ec2a6f201b`, targeting `d3f4338a7735ff610947c1232ebf7797f584993d`. Draft release ID `384291529` remains unpublished with no assets. No Docker Hub promotion or Pages deployment has occurred.

After demo approval, verify the actual merged main SHA and next workflow run number, then issue a new exact-run preflight lease with the existing coordinator. Keep the trusted-main guard. Reconcile verified receipts before separately leasing the full remediation; retain any unknown usage. Only a passing full run permits the provider-free reconciliation test and conditional release sequence. Do not repeat an unchanged failing semantic probe.

After source-specific tag approval and complete live evidence, the existing release helper can attach the already verified signed bundle without rebuilding it. It requires checkout HEAD and tag to equal the tested source, verifies the exact-source/signer attestation and existing asset bytes, and refuses published releases or mismatched assets. Update the empty draft's notes to the correct source/preparation before publication. Actual publication must verify promotion of those exact OCI bytes, then the demo must execute full remediation using the published tooling index digest and verify expected PR reuse. A simple echo is insufficient for this final gate.

Useful read-only continuation commands:

```sh
gh pr view 7 --repo Ompragash/agentctl-remediation-demo
gh pr view 7 --repo opensourceops/opensourceops.github.io
gh run view 34164848388 --repo opensourceops/agentctl
gh api repos/opensourceops/agentctl/releases/384291529
```

The original user checkout and its pre-existing CLI/documentation changes are preserved. Production publication remains blocked, and the complete release verdict remains **not ready** until the outstanding live, approval and published-consumer gates pass.
