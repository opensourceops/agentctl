# Documentation examples

These examples support public guides. `cargo xtask docs-verify` checks every YAML file and executes the credential-free examples from temporary directories.

| Journey | Provider | Verification | Network | Security note |
| --- | --- | --- | --- | --- |
| `release-readiness` | fake | mock-backed run with deterministic gates | none | model result cannot override failed assertions |
| `scheduled-review` | none | deterministic run and artifact | none | scheduler owns overlap and retention |
| `ci-quality-gate` | none | deterministic success and failure paths | none | pipeline uses the exit code |
| `provider-portability/fake.yaml` | fake | deterministic run | none | learning default |
| `provider-portability/openai.yaml` | OpenAI | static validation only | opt-in live | credential is an environment reference |

Prominent documentation examples should use source inclusion from these files or existing checked examples. Do not copy and edit a second YAML block in the site repository.
