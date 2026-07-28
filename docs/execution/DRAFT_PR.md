# Draft pull request

## Title

Harden bounded process execution and enable hosted `v1alpha1` RC gates

## Summary

- bound shell-action stdout, stderr, and combined capture with validated workflow/pack limits, concurrent draining, termination/reaping, secret-safe diagnostics, and durable timeout/cancellation semantics;
- bound repository acceptance/container command capture and preserve parseable JSON behavior;
- add automatic Linux x64, macOS arm64, Windows x64, container, dependency, complete-history/tree secret, workflow-lint, package, vulnerability, and SBOM gates;
- pin every external action to a reviewed full commit SHA with an exact-version comment;
- add an optional build-only CA secret mount without disabling TLS or retaining the CA in image layers/history;
- document the hosted handoff, required checks, artifacts, digests, and release procedure.

## Local verification

- `cargo xtask verify`
- `cargo xtask acceptance`
- `cargo xtask package`
- `AGENTCTL_BUILD_CA_FILE=<protected-public-CA-bundle> cargo xtask acceptance-container`
- actionlint 1.7.12: passed
- Gitleaks 8.30.1 complete history and current tracked tree: no findings; synthetic credential: detected
- Trivy 0.72.0: zero fixed HIGH/CRITICAL findings; CycloneDX image SBOM validated

All credential-free gates were run with provider credential variables removed. No live provider call was made. `.release-evidence` was not read, modified, staged, scanned as tree content, or uploaded.

## Hosted validation evidence

The exact-head three-platform, production SBOM, container, and supply-chain
checks have run successfully. The manual `rc-release-preparation` workflow
also passed on the same candidate and retained all three package artifacts.
The independent candidate report records immutable run IDs and artifact
digests. Any later source change invalidates that evidence and requires the
applicable exact-head gates to run again.

## Risk and review focus

- subprocess termination and simultaneous stdout/stderr pressure;
- failed-versus-uncertain durable effect classification;
- Windows compilation and acceptance behavior;
- action/Syft/Trivy/Gitleaks version and SHA review;
- optional CA cleanup and absence from build history/artifacts;
- no accidental `.release-evidence` artifact inclusion.
