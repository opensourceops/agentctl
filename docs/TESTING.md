# Testing and verification

The canonical command is:

```console
cargo xtask verify
```

User-journey layers are separate:

```console
cargo xtask acceptance
cargo xtask acceptance-container
cargo xtask examples-verify
cargo xtask devops-examples
cargo xtask package
cargo xtask secret-scan
```

It checks rustfmt; clippy with all targets/features and warnings denied; locked build; unit, integration, compatibility, provider, protocol, persistence, runtime, and security tests; rustdoc; generated schema/CLI consistency; all workflow validation and deterministic examples; negative capability/policy/no-mutation cases; dependency sources/licenses/advisories; repository secret patterns and immutable workflow action pins; `cargo install`; and the Rust-only production boundary.

Unit tests cover parser diagnostics, strictness, compiler order/cycles/capabilities, templates, tool schemas, policy traversal/network/redaction, state transitions, effect recovery, store migration/corruption/checkpoints, runtime dataflow/check/diff/approval/cancellation/replay/repair/fork, provider mappings, protocols, and traces. Repair regressions cover two-agent reuse with a panic-on-repeat provider, downstream and branch closure, repeated roots, changed definitions/prompts, output/state/artifact corruption, migration and rollback, effect uncertainty/reconciliation, approval gating, source garbage collection, and effect-free replay. `proptest` exercises arbitrary templates and typed preservation. Language-neutral fixtures in `fixtures/compat` preserve the TypeScript oracle’s external graph/dataflow contract.

`fuzz/` contains `cargo-fuzz` targets for workflow YAML/templates, provider responses, MCP/A2A payload shapes, persisted state, and tool schemas/inputs. They use no network or credentials. Example:

```console
cargo install cargo-fuzz
cargo fuzz run workflow_yaml -- -max_total_time=60
```

Hosted CI runs the canonical suite, credential-free acceptance, and packaging
on Rust 1.88 for Linux x64, macOS arm64, and Windows x64. Separate automatic
jobs cover the Linux x64 container, current vulnerability scan, two CycloneDX
SBOM artifacts, complete-history/tree secret scans, dependency policy, and
workflow lint. Exact-head pull-request and release-preparation runs provide
validated hosted-platform evidence. Provider/protocol conformance uses local
mock HTTP servers. Normal examples are deterministic; MCP/A2A runtime behavior
is covered by mocks rather than requiring a background service.

## Explicit paid gates

Ordinary verification requires no provider credentials. Live gates require the
runtime credential, verified model access, and one persistent absolute SQLite
budget path shared by the complete suite and every retry. Configure the
allowance once, retain it across commands, and do not create a new ledger to
bypass exhausted or uncertain reservations:

```sh
export AGENTCTL_LIVE_BUDGET="$PWD/.agentctl/launch-live-budget.sqlite3"
export AGENTCTL_LIVE_MODEL=gpt-5.6-sol
cargo xtask acceptance-live-openai
cargo xtask resource-budget-live-openai
cargo xtask examples-verify-live-openai
```

These commands are a deliberate release sequence, not a debugging loop. The
first gate performs the tool-call/continuation journey locally and in the OCI
image, so it requires a usable container engine and image prerequisites. The
resource gate allows one dispatch and proves that the next requested effect
is denied. The legacy example gate inventories public OpenAI workflows and
the failed two-agent source/selective repair/keyless replay journey. It retains
its additional 40-request and conservative US$10 guard. The four distinct
DevOps OpenAI workflows are examples 01, 12, 19, and 20 and are included in the
example gate; the other examples
remain deterministic. See their [catalog](../examples/devops/catalog.json).

After a failure in the composite, `cargo xtask examples-verify-live-openai-composites`
runs that composite, selective repair, the OCI repair case and the four DevOps
variants. It preserves every semantic assertion while avoiding four independent
legacy workflows that already passed. Its evidence lists only executed examples
and marks `legacyInventoryComplete: false`; combine it with the earlier source,
logs and charged ledger when assessing the complete inventory. The normal full
gate still executes every workflow. The container-only continuation remains
available after a completed local summary. Do not repeat a successful paid case
just to assemble a single green command invocation.

For the four DevOps variants alone, use their runner's `--mode openai --model
gpt-5-mini --live-budget "$AGENTCTL_LIVE_BUDGET" --keep` options and a report path;
this is an alternative to their execution within the full example gate.

The shared allowance is at most 100 provider requests, 200,000 input-plus-output
tokens, 1,800 seconds of paid execution, and US$25 estimated cost, including
retries. SQLite transactions reserve every workflow's request/token/time/cost
upper bounds before dispatch, including local and OCI invocations. Complete
durable usage reconciles the reservation after success or definitive failure.
Reasoning tokens are already part of output tokens and are not counted twice.
Unknown usage or process death retains the whole reservation and fails the
gate; it is never silently converted to a passing result. Parallel callers
share the same atomic allowance; accumulated paid execution time may be more
conservative than elapsed suite time.

The CLI wrapper retains run identity, numeric observed budget counters and
effect statuses in the shared ledger before attempting reconciliation. It
excludes prompts, workflow values, provider error text and tool/model outputs.
This preserves actionable accounting evidence when a temporary fixture is
cleaned up after failure.

Copied live fixtures receive the explicitly selected model, bounded runtime
budgets, and a versioned price schedule before compilation and approvals.
Per-agent output reservations also fit the existing aggregate output ceiling
at the configured concurrency; the harness does not enlarge that ceiling.
Checked-in model choices remain unchanged. Monetary accounting is an estimate
from reviewed public prices, not an invoice or automatic price discovery.
Unpriced model selections fail before dispatch. The lower-cost DevOps variants
explicitly use `gpt-5-mini`; selecting the coding model does not select the
model used by these gates. See [Providers](PROVIDERS.md).

Credential-free harness regressions are:

```sh
python3 -m unittest discover -s examples/devops -p test_live_budget.py -v
python3 -m unittest discover -s scripts -p test_live_command.py -v
python3 -m unittest discover -s scripts -p test_container_agentctl.py -v
```

The [execution ledger](execution/AUTONOMOUS_LAUNCH_READINESS.md) records the
actual code-under-test SHA, model IDs, requests, tokens, estimates, and results.
Historical live evidence is not certification of a changed candidate. Never
run paid gates for fuzzing, load, or ordinary CI.
