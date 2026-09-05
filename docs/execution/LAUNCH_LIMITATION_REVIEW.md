# Current launch limitation review

Reviewed 2026-09-06. This note classifies the expanded launch task against the
implemented product boundary. Exact code-under-test commits, commands,
failures, live usage, and hosted artifacts belong to the
[execution ledger](AUTONOMOUS_LAUNCH_READINESS.md). A historical verified row
does not substitute for current-commit evidence.

## Gaps found in the continuation

| Area | Classification | Resolution and current evidence contract |
| --- | --- | --- |
| Instruction sources absent from check/plan capture and task-context template evaluation | Implementation defect | Capture bounded bytes before compilation, render instructions through typed task context, and bind effect/reuse checks to captured content. New captured histories support no-reread recovery. |
| Ordered workflow/agent/task variable files and visible final precedence | Developer-experience gap | Add `varsFiles`, explicit invocation overrides, layer/effective origin diagnostics, and a separate typed-input namespace. The [variables guide](../VARIABLES.md) includes an executed credential-free example. |
| Pack instruction/variable content not covered by a manifest asset declaration | Implementation defect | Require declared file digests and verify captured bytes against the locked manifest. Caller workspace/read policy still applies. |
| Remote pack acquisition outside the ordinary network-policy path | Implementation defect | Require caller URL/resolved-IP policy for fresh archives, bounded pinned clients, and explicit proxy permission. Reject fresh HTTPS Git transport until equivalent DNS pinning is enforceable; permit contained local Git and verified caches. See [Packs](../PACKS.md) and final negative evidence in the execution ledger. |
| Windows configuration reads vulnerable to intermediate junction swap/restore | Implementation defect | Replace pathname pre/post checks with no-follow handle-relative component opens and retain ancestors. Sharing-only guards do not stop attribute-only reparse mutation. Three deterministic race/retention tests plus the static junction test compile on Rust 1.88; hosted Windows execution is still required. See the [feature matrix](FEATURE_EVIDENCE.md). |
| Provider reusing a tool-call ID across separate model effects | Implementation defect | Schema migration 16 scopes storage by run, model effect, and raw provider call ID; continuation keeps raw IDs. |
| Doctor claiming readiness from an executable or an unexecuted process secret | Developer-experience gap | Bound engine/image inspection, distinguish unchecked process references, redact values, and return a failing readiness exit when prerequisites remain unverified. |
| Shared live suite limits enforced only per command or after execution | Implementation defect in validation harness | Reserve aggregate requests, tokens, wall time, and estimated cost before dispatch; reconcile complete durable usage, retain uncertain reservations, and avoid double-counting reasoning tokens. |
| Live CI-diagnosis response accepted a schema wider than its deterministic verifier | Implementation defect in example contract | Bind classification to the documented enum and reject malformed classifications before downstream verification. Focused deterministic assertions passed; the paid attempt remains failed and the four distinct new live workflows remain a release gate. |
| Twenty useful DevOps workflows with one discoverable runner | Developer-experience gap | The [catalog](../../examples/devops/catalog.json), [suite guide](../../examples/devops/README.md), and [machine-readable evidence](../../examples/devops/validation.json) distinguish deterministic, fake-provider, local-service, and OpenAI execution. Final-source and live gates remain explicit. |
| Repeated public source footer and stale paired-source documentation | Developer-experience gap | Fix the site's importer, retain source paths/routes/digests as metadata, and verify the exact clean framework checkout with the paired workflow pin. |
| New-source live model, platform, image, package, security, SBOM, and site results | Environmental evidence gap | Execute the required local/hosted gates on the final source; report unavailable or failed gates without relabeling historical or simulated results. |

## Retained limitation classifications

| Boundary or historical register IDs | Classification | Retained decision |
| --- | --- | --- |
| Bounded local scheduling and static graph expansion: SCH-001, DYN-001, COND-001, LOOP-001, SUB-001 | Intentional product boundary | Local bounded graphs, constrained expressions, explicit dependencies, fixed loop ceilings, and typed sub-workflow boundaries remain authoritative. |
| Effect certainty, retry, repair, compensation, and legacy recovery: EFX-001, RET-001, COMP-001, MIG-001 | Intentional product boundary | Ambiguous non-idempotent effects require reconciliation. Repair reuses only proven boundaries; compensation is best-effort inverse execution. Unrecorded legacy source content cannot be invented. |
| Artifact, encryption, and memory bounds: ART-001, ENC-001, MEM-001 | Intentional product boundary | Keep local CAS and SQLite ownership, explicit selected-field encryption, bounded ingestion/search, and explicit promotion. Encryption does not cover all metadata or artifact bytes. |
| Host/process/container authority: ISO-001, EXT-001, NET-001, BUD-001 | Intentional product boundary | Allowlists are not an OS sandbox; container mode trusts its engine/image. Explicit network grants and conservative reservations remain required. Pricing is versioned operator input. |
| Structured collaboration: TEAM-001 | Intentional product boundary | Typed graph handoffs replace hidden free-form conversation control flow. |
| MCP/A2A/streaming continuation limits: MCP-001, A2A-001, STR-001, PROV-001 | Intentional product boundary | Bound reconnect and stream retention; do not resubmit an ambiguous remote mutation or change raw provider continuation IDs. |
| Pack registry/discovery and trust-root lifecycle: PACK-001, TRUST-001, REG-001 | Intentional product boundary | Explicit pinned sources and locks remain required. No hosted registry or automatic version discovery is introduced; installed trust material has an explicit lifecycle. |
| External secret managers, pricing discovery, or specialized memory indexes: SEC-001 and optional MEM-001 adapters | Optional integration | Environment/file/process secret references and public provider/memory contracts remain the supported extension paths. Ordinary variable files are not a secret store. |
| Event triggers, calendars, distributed storage/scheduling, hosted UI/chat: EVENT-001, DIST-001, UI-001 | Intentional product boundary | External systems own scheduling and production environments; one local process and SQLite history remain the correctness boundary. |
| Linux/macOS/Windows and OCI observations: XPLAT-001, OCI-001 | Environmental evidence gap for each new candidate | Preserve separate exact-source platform, image-digest, package, vulnerability, and SBOM results. Local arm64 results do not establish hosted x64 behavior. |
| Non-OpenAI providers, real MCP/A2A peers, cloud and production deployment | Environmental evidence gap | Native/mock coverage is labeled honestly. A configured OpenAI credential or local deployment fixture does not establish these services. |

## Release interpretation

The older [thirty-item burn-down](LIMITATION_BURNDOWN.md) remains useful
implementation history. The continuation adds source capture, precedence,
pack input integrity, tool-call identity, suite accounting, and developer
journeys; those changes have their own validation obligations. Preserve a
not-ready verdict until all required final-source gates pass. A successful
pre-1.0 candidate is not an automatic stable 1.0 release.
