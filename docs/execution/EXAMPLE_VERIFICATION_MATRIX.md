# Example verification matrix

This inventory is enforced by `cargo xtask examples-verify`. The default command is credential-free: it discovers every YAML file under `examples/` and `fixtures/compat/`, requires one row per file, runs `check` and `plan` with the documented exit codes, and then runs the repository's canonical deterministic and mock journeys. `cargo xtask examples-verify-live-openai` is the explicit bounded live gate for every row marked OpenAI.

`N/A` means the check does not apply to that example. `Canonical` means the behavior is exercised by the existing deterministic example or acceptance runner. `Opt-in` means credentials or an external service are required and the default gate does not claim execution.

| Path | Purpose | Provider | Expected status | Check exit | Plan exit | Deterministic run | Mock run | Live run | Container run | Artifact verification | Output verification | Last deterministic result |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- | --- | --- | --- |
| `examples/acceptance/mock-tool/workflow.yaml` | Full fake-provider tool journey | fake | success | 0 | 0 | Canonical | Canonical | N/A | Canonical | Canonical | Canonical | passed |
| `examples/custom-pack-tools/custom.pack.yaml` | Legacy custom pack manifest | N/A | pack manifest | 0 | 0 | N/A | N/A | N/A | N/A | Static | Static | inventoried |
| `examples/custom-pack-tools/mission.playbook.yaml` | Archived TypeScript custom-pack example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/dataflow/mission.playbook.yaml` | Archived TypeScript dataflow example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/demo.pack.yaml` | Legacy demonstration pack manifest | N/A | pack manifest | 0 | 0 | N/A | N/A | N/A | N/A | Static | Static | inventoried |
| `examples/docs/ci-quality-gate/workflow.yaml` | CI quality decision | deterministic | success and expected failure | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Canonical | passed |
| `examples/docs/provider-portability/fake.yaml` | Portable fake-provider summary | fake | success | 0 | 0 | Canonical | Canonical | N/A | N/A | N/A | Canonical | passed |
| `examples/docs/provider-portability/openai.yaml` | Portable OpenAI summary | OpenAI | success | 0 | 0 | N/A | N/A | Passed 2026-07-23 | N/A | N/A | Live gate | live passed |
| `examples/docs/release-readiness/workflow.yaml` | Release evidence review | fake | success | 0 | 0 | Canonical | Canonical | N/A | N/A | N/A | Canonical | passed |
| `examples/docs/scheduled-review/workflow.yaml` | Scheduled deterministic review | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | Canonical | Canonical | passed |
| `examples/hello.playbook.yaml` | Archived TypeScript hello example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/memory-flow/mission.playbook.yaml` | Archived TypeScript memory example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/openai-live/workflow.yaml` | Canonical OpenAI tool continuation | OpenAI | success | 0 | 0 | N/A | Protocol mock | Passed 2026-07-23 | Blocked: Podman unavailable | Canonical | Canonical | live passed; container blocked |
| `examples/prompt-cache/mission.playbook.yaml` | Archived TypeScript prompt-cache example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/prompt-file-vars/mission.playbook.yaml` | Archived TypeScript prompt-file example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/real-autonomy/mission.playbook.yaml` | Archived TypeScript autonomy example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/remote-mcp-autonomy/mission.playbook.yaml` | Archived TypeScript MCP example | legacy | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON error | passed |
| `examples/selective-repair-openai/repaired.workflow.yaml` | Fixed two-agent repair target | OpenAI | success | 0 | 0 | Runtime mock | Runtime mock | Passed 2026-07-23 | Blocked: Podman unavailable | Marker artifact verified | Contract and marker | live passed; container blocked |
| `examples/selective-repair-openai/source.workflow.yaml` | Deliberately failed two-agent source | OpenAI | task 2 failure | 0 | 0 | Runtime mock | Runtime mock | Expected failure passed 2026-07-23 | Blocked: Podman unavailable | N/A | Durable task 1 output | live passed; container blocked |
| `examples/v1/a2a.yaml` | A2A delegation contract | A2A | external execution | 0 | 0 | Protocol mock | Protocol mock | N/A | N/A | N/A | Static | passed |
| `examples/v1/anthropic-live.yaml` | Anthropic native provider | Anthropic | credentialed execution | 0 | 0 | N/A | Protocol mock | External opt-in | N/A | N/A | Static | passed |
| `examples/v1/approval.yaml` | Approval-paused mutation | deterministic | paused | 0 | 0 | Acceptance equivalent | N/A | N/A | N/A | No write before approval | Canonical | passed |
| `examples/v1/capability-failure.yaml` | Negative capability contract | deterministic | validation failure | 2 | 2 | Expected failure | N/A | N/A | N/A | N/A | JSON diagnostics | passed |
| `examples/v1/check-diff.yaml` | Non-mutating check and diff | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | No mutation | Canonical | passed |
| `examples/v1/condition.yaml` | Conditional scheduling | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Canonical | passed |
| `examples/v1/crash-resume.yaml` | Durable interruption fixture | fake | resumable | 0 | 0 | Acceptance equivalent | Canonical | N/A | N/A | N/A | Durable state | passed |
| `examples/v1/dataflow.yaml` | Typed task dataflow | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Canonical | passed |
| `examples/v1/example.pack.yaml` | Native reusable pack manifest | N/A | pack manifest | 0 | 0 | Canonical consumer | N/A | N/A | N/A | Digest checked | Canonical | passed |
| `examples/v1/fake-provider.yaml` | Deterministic provider task | fake | success | 0 | 0 | Canonical | Canonical | N/A | N/A | N/A | Canonical | passed |
| `examples/v1/google-live.yaml` | Google native provider | Google | credentialed execution | 0 | 0 | N/A | Protocol mock | External opt-in | N/A | N/A | Static | passed |
| `examples/v1/hello.yaml` | Minimal assign workflow | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Canonical | passed |
| `examples/v1/long-term-memory.yaml` | Namespaced durable memory | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | SQLite | Canonical | passed |
| `examples/v1/loop.yaml` | Bounded durable loop and ordered iteration aggregation | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Iteration states and values | passed |
| `examples/v1/matrix.yaml` | Bounded static matrix and ordered aggregation | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Aggregate states and values | passed |
| `examples/v1/mcp.yaml` | MCP call contract | MCP | external execution | 0 | 0 | Protocol mock | Protocol mock | N/A | N/A | N/A | Static | passed |
| `examples/v1/openai-live.yaml` | Minimal OpenAI response | OpenAI | success | 0 | 0 | N/A | Protocol mock | Passed 2026-07-23 | N/A | N/A | Live gate | live passed |
| `examples/v1/parallel.yaml` | Deterministic parallel batch | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | Atomic ordered memory merge | Canonical | passed |
| `examples/v1/policy-denial.yaml` | Denied mutation | deterministic | policy failure | 0 | 0 | Canonical expected failure | N/A | N/A | N/A | No mutation | JSON error | passed |
| `examples/v1/reusable-pack.yaml` | Native reusable pack consumer | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | Pack digest | Canonical | passed |
| `examples/v1/router.yaml` | Typed deterministic route selection | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Decision and skipped branch | passed |
| `examples/v1/secret-reference.yaml` | Environment reference contract | OpenAI | success | 0 | 0 | N/A | Protocol mock | Passed 2026-07-23 | N/A | N/A | Secret-safe live gate | live passed |
| `examples/v1/subworkflow.yaml` | Typed reusable namespaced graph | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | N/A | Typed input/output boundaries | passed |
| `examples/v1/working-memory.yaml` | Working-memory update | deterministic | success | 0 | 0 | Canonical | N/A | N/A | N/A | SQLite | Canonical | passed |
| `fixtures/compat/v0/assign.playbook.yaml` | Language-neutral TypeScript compatibility fixture | legacy translator | success | 0 | 0 | Compatibility test | N/A | N/A | N/A | N/A | Oracle contract | passed |

The live column is updated only by the opt-in command. Raw model content, databases, and credentials are never written to this committed matrix.
