# Product definition

## Thesis and boundaries

`agentctl` is a local-first control plane for automation that needs ordinary deterministic work and narrowly bounded model reasoning in the same durable run. The graph, policy, persistence, and replay are authoritative; a model is a replaceable executor for one task.

Primary users are application and platform engineers authoring reviewed automation, security-conscious teams introducing model calls into existing operations, CI maintainers needing credential-free validation, and Rust applications embedding the runtime. Their jobs are to validate before acting, understand an exact plan, constrain effects, recover from interruption, prove what happened, and reuse reviewed content.

Core use cases are local repository automation, approval-gated changes, structured model enrichment, provider-portable agent tasks, selective repair from a failed task boundary, MCP tool calls, A2A delegation, cron-invoked runs, and generic containerized CI steps. `agentctl` is a schedulable runtime, not a scheduler: cron, systemd, Kubernetes, and CI own triggers and overlap policy. Hosted orchestration, a visual builder, chat, distributed scheduling, a public registry, arbitrary configuration management, secret storage, and unbounded autonomy are non-goals.

## Journeys

- Local: author strict YAML, run `check`, inspect `plan`, preview with `run --check --diff`, execute, approve if required, and inspect the audit history.
- Scheduled: invoke the CLI without a TTY, use explicit database/workspace/artifact paths and an overall timeout, receive exit `3` for a durable pending approval, and resume through an operator-controlled invocation.
- CI: mount config/workspace/state/artifacts into the generic OCI image, inject secrets only as environment variables, pass inputs by `--inputs-file` or repeated `--input`, and consume one versioned final JSON envelope on stdout.
- Embedded: construct core workflow and plan values, inject a store, providers, tools, clock, IDs, and tracing, then invoke the runtime with a cancellation token.
- Repair: keep the failed terminal source immutable, compile a corrected target, plan one or more roots, reuse compatible successful boundaries, and execute only the roots and their affected descendants.

Provider portability means the internal message, tool, continuation, usage, and capability contracts do not expose provider SDK types. It does not mean every provider has identical features. Compilation rejects a requested feature absent from the chosen provider.

Reusable packs have a versioned manifest, fully qualified name, semantic version, agentctl constraint, capability/provider declarations, and file integrity verification. This release deliberately has no remote registry or executable plugin ABI.

## Trust model

Workflow and pack authors are trusted to request work, but their requests remain policy constrained. Model output, tool output, remote descriptions, file content, MCP annotations, A2A cards, and network responses are untrusted. Environment variables may contain secrets and are read only at adapter boundaries after allowlist checks. Primary provider credentials are loaded immediately before dispatch; configured header references are loaded during adapter construction, before run creation. SQLite is local durable state, not a secret vault.

## Compatibility and maturity

The current document API is `agentctl.dev/v1alpha1`; breaking changes may occur with explicit diagnostics and migration support. Machine output, plan, effects, runtime state, checkpoints, database schema, audit events, and protocol continuation all carry independent versions. Deprecations are documented for at least one compatibility window; incompatible durable state fails explicitly.

Version 0.2 is a production-oriented alpha with executable evidence for the stated local, scheduled, and generic-container journeys. The workflow schema remains `v1alpha1`, so callers must pin the binary/image version. A stable release requires a frozen v1 workflow schema, accumulated cross-platform CI history, documented long-horizon database upgrade support, expanded compatibility fixtures, and a security review of any newly added executor.

## Differentiation

This is not a chat-agent or multi-agent conversation framework: workflows, not conversations, own control flow. It is not CI/CD: it can run inside CI but does not manage runners or deployment environments. It borrows idempotence and check/diff vocabulary from Ansible without becoming configuration management. It borrows plan/effect separation from Terraform without owning infrastructure state. It is not a hosted orchestrator or general scripting language: one local process, SQLite, constrained templates, typed actions, and explicit remote effects are intentional boundaries.

The differentiator is the combination of deterministic compilation, honest predictability, durable effect identity, recorded no-effect replay, compatibility-checked task-boundary repair, native provider portability, and policy decisions made outside the model.
