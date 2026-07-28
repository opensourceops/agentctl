# Security policy

## Supported line

The current supported release line is agentctl `0.3` with workflow API
`agentctl.dev/v1`. The workflow API is stable; the CLI and crates remain
pre-1.0 and do not carry a long-term support promise.

## Report a vulnerability privately

Use GitHub's private vulnerability reporting flow for `opensourceops/agentctl` when it is enabled. Do not open a public issue with exploit details. If the private form is unavailable, contact the repository owners through a private channel listed on the OpenSourceOps GitHub organization profile before sending sensitive details.

Include the affected commit or version, impact, minimal reproduction, and suggested mitigation. Remove credentials, production prompts, database contents, and confidential artifacts. Use clearly fake values in every reproduction.

The maintainers do not promise a response or remediation deadline. They will assess reports against the implemented trust boundary and coordinate disclosure when appropriate.

## Public hardening questions

Questions about documented boundaries that do not disclose a vulnerability may use a GitHub discussion or issue. Read [Security](docs/SECURITY.md), [Threat model](docs/THREAT_MODEL.md), and [Limitations](docs/LIMITATIONS.md) first.
