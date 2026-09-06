# Remediation implementation contract

This independent package targets one direct Python dependency (`urllib3`) and only its two reviewed dependency files. The sample deliberately starts at 2.6.2; 2.7.0 is verified against the upstream advisories and PyPI wheel metadata. No Trivy pass, live model call, image availability or PR publication is implied by those metadata checks.

1. A trusted runner captures an application image, source commit/tree, a real Trivy JSON report, and one fixed vulnerability database snapshot.
2. A single agentctl run invokes Astra analysis and implementation through a typed validated handoff. Agent B can write exactly the two approved staging files using existing bounded workspace tools. It receives no Docker socket or GitHub credential and cannot change policy, reports, tests, or workflow source.
3. Deterministic validation binds the staged patch to the source and report. Trusted outer steps test, rebuild and rescan the changed tree using the identical database bytes. These outer effects are outside that agentctl run's replay boundary.
4. A separate credential-free agentctl workflow validates captured gate identities and decides publication eligibility. A fresh trusted publisher job reconciles branches/PRs in the explicitly configured demo repository; it never infers a destination from model output.

Stable source+patch identity drives deduplication. Base movement invalidates publication; no rebase is performed against stale test evidence. Uncertain API responses are reconciled by reads before any further mutation. Replay reconstructs each recorded agentctl boundary without rerunning models, patch tools, builds, scans or GitHub operations. A fresh CI runner is a new invocation with publication reconciliation, not a claimed resume.

Scanner/base image digests and action pins are checked into the package. The exact reviewed framework SHA is supplied by the explicit export command; a source/run/job-bound shared-budget lease is supplied by the trusted coordinator before paid dispatch. These are deployment configuration inputs, never model choices. The production Docker Hub digest is configured only after publication.
