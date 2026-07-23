# Write and verify documentation

## Ownership

The `agentctl` repository owns commands, schema, examples, runtime behavior, provider and protocol capability, security, architecture, limitations, and contributor contracts. The `opensourceops.github.io` repository owns the public homepage, navigation, learning paths, search, styling, metadata, browser tests, and Pages deployment.

Do not hand-copy a prominent YAML example into the site repository. Add or update a checked example here and include it through the Pages import mechanism.

## Add an example

Place public journeys in `examples/docs/` with a README entry, provider classification, expected result, verification command, network requirement, and security note. Use fake providers or local mocks for normal verification. Name live examples clearly and keep them opt-in.

## Write public content

- Use sentence-case headings, active voice, short paragraphs, and descriptive links.
- State the working directory, writes, credentials, and network effect for commands.
- Use exact commands and complete valid YAML.
- Distinguish implemented, deterministic, mock-tested, live evidence, hosted configuration, hosted execution, and deferred work.
- Never use an em dash in public copy.
- Avoid hype, generic AI claims, and unsupported maturity language.
- Explain every Mermaid diagram before and after it.

## Verify locally

From this repository:

```text
cargo xtask docs-verify
```

From the Pages repository:

```text
AGENTCTL_REPO=/path/to/agentctl pnpm verify:agentctl
```

The site command imports canonical content, records the source commit, validates writing and links, checks Mermaid, builds search, assembles the physical `/agentctl/` artifact, then runs browser and accessibility tests.

## Review

Review technical claims against source, generated help, schema, tests, and evidence. Check keyboard navigation, heading order, link purpose, alt text, diagram explanations, mobile code blocks, and direct deep links. Do not claim accessibility certification from automated tooling alone.
