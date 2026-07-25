# CLI reference

Generated from the Rust CLI by `cargo xtask generate`. Do not edit by hand.

## `agentctl`

```text
Deterministic, declarative control plane for policy-constrained agentic automation

Usage: agentctl [OPTIONS] <COMMAND>

Commands:
  check       Validate syntax, schema, references, capabilities, policy, and templates
  plan        Print the deterministic compiled plan
  run         Execute a workflow, or predict it with --check
  resume      Continue an interrupted or approval-paused run
  replay      Reconstruct a terminal run only from recorded state and results
  fork        Create a new run from a prior workflow with fresh effects
  repair      Create a new run that reuses compatible upstream results and executes a repaired suffix
  retry       Retry failed or selected boundaries of an identical terminal workflow
  compensate  Execute explicitly declared best-effort compensation for a terminal run
  runs        Analyze or upgrade retained legacy run records for selective reuse
  cancel      Durably request cancellation
  inspect     Inspect durable run, task, and audit state
  effects     Inspect or narrowly reconcile uncertain effects
  approvals   List or resolve durable approval requests
  providers   Inspect provider capabilities or run the opt-in OpenAI smoke
  auth        Check configured secret references without revealing values
  schema      Print or write the generated workflow JSON Schema
  migrate     Translate an unversioned TypeScript-era workflow into v1alpha1
  packs       Inspect and verify a local reusable pack
  artifacts   Inspect, verify, export, or collect durable artifacts
  db          Inspect the runtime database
  memory      Read or write namespaced long-term memory
  gc          Garbage-collect expired memory and old terminal runs
  completion  Generate completion for a supported shell
  version     Print the exact build version
  update      Explain safe update options without modifying the installation

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
  -V, --version          Print version
```

## `agentctl check`

```text
Validate syntax, schema, references, capabilities, policy, and templates

Usage: agentctl check [OPTIONS] <FILE>

Arguments:
  <FILE>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl plan`

```text
Print the deterministic compiled plan

Usage: agentctl plan [OPTIONS] <FILE>

Arguments:
  <FILE>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl run`

```text
Execute a workflow, or predict it with --check

Usage: agentctl run [OPTIONS] <FILE>

Arguments:
  <FILE>

Options:
      --db <DB>                            [default: .agentctl/runtime.db]
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --inputs <INPUTS>
      --inputs-file <INPUTS_FILE>
      --verbose
      --input <KEY=VALUE>
      --workspace <WORKSPACE>
      --timeout-seconds <TIMEOUT_SECONDS>
      --check
      --diff
      --interactive
  -h, --help                               Print help
```

## `agentctl resume`

```text
Continue an interrupted or approval-paused run

Usage: agentctl resume [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --db <DB>                            [default: .agentctl/runtime.db]
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --diff
      --interactive
      --verbose
      --workspace <WORKSPACE>
      --timeout-seconds <TIMEOUT_SECONDS>
  -h, --help                               Print help
```

## `agentctl replay`

```text
Reconstruct a terminal run only from recorded state and results

Usage: agentctl replay [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl fork`

```text
Create a new run from a prior workflow with fresh effects

Usage: agentctl fork [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --db <DB>                            [default: .agentctl/runtime.db]
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --interactive
      --diff
      --verbose
      --workspace <WORKSPACE>
      --timeout-seconds <TIMEOUT_SECONDS>
  -h, --help                               Print help
```

## `agentctl repair`

```text
Create a new run that reuses compatible upstream results and executes a repaired suffix

Usage: agentctl repair [OPTIONS] --from <FROM> <FILE> <SOURCE_RUN_ID>

Arguments:
  <FILE>
  <SOURCE_RUN_ID>

Options:
      --from <FROM>
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --plan
      --restart-successful
      --verbose
      --reason <REASON>
      --db <DB>                            [default: .agentctl/runtime.db]
      --interactive
      --diff
      --workspace <WORKSPACE>
      --timeout-seconds <TIMEOUT_SECONDS>
  -h, --help                               Print help
```

## `agentctl retry`

```text
Retry failed or selected boundaries of an identical terminal workflow

Usage: agentctl retry [OPTIONS] <FILE> <SOURCE_RUN_ID>

Arguments:
  <FILE>
  <SOURCE_RUN_ID>

Options:
      --failed
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --from <FROM>
      --plan
      --verbose
      --restart-successful
      --reason <REASON>
      --db <DB>                            [default: .agentctl/runtime.db]
      --interactive
      --diff
      --workspace <WORKSPACE>
      --timeout-seconds <TIMEOUT_SECONDS>
  -h, --help                               Print help
```

## `agentctl runs`

```text
Analyze or upgrade retained legacy run records for selective reuse

Usage: agentctl runs [OPTIONS] <COMMAND>

Commands:
  analyze  Prove reusable legacy metadata without changing the source run
  upgrade  Transactionally persist every legacy field that can be proven

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl runs analyze`

```text
Prove reusable legacy metadata without changing the source run

Usage: agentctl runs analyze [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl runs upgrade`

```text
Transactionally persist every legacy field that can be proven

Usage: agentctl runs upgrade [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --dry-run
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl cancel`

```text
Durably request cancellation

Usage: agentctl cancel [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl inspect`

```text
Inspect durable run, task, and audit state

Usage: agentctl inspect [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl effects`

```text
Inspect or narrowly reconcile uncertain effects

Usage: agentctl effects [OPTIONS] <COMMAND>

Commands:
  list
  inspect
  reconcile

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl effects list`

```text
Usage: agentctl effects list [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --task <TASK>
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl effects inspect`

```text
Usage: agentctl effects inspect [OPTIONS] <EFFECT_ID>

Arguments:
  <EFFECT_ID>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl effects reconcile`

```text
Usage: agentctl effects reconcile [OPTIONS] --status <STATUS> --reason <REASON> <EFFECT_ID>

Arguments:
  <EFFECT_ID>

Options:
      --output <OUTPUT>
          [default: human] [possible values: human, json]
      --status <STATUS>
          [possible values: applied, not-applied, compensated]
      --actor <ACTOR>
          [default: cli-user]
      --color <COLOR>
          [default: auto] [possible values: auto, always, never]
      --reason <REASON>

      --verbose

      --evidence-file <EVIDENCE_FILE>

      --result-file <RESULT_FILE>

      --result-schema-file <RESULT_SCHEMA_FILE>

      --compensation-effect <COMPENSATION_EFFECT>

      --approved

  -h, --help
          Print help
```

## `agentctl approvals`

```text
List or resolve durable approval requests

Usage: agentctl approvals [OPTIONS] <COMMAND>

Commands:
  list
  approve
  reject

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl approvals list`

```text
Usage: agentctl approvals list [OPTIONS] <RUN_ID>

Arguments:
  <RUN_ID>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl approvals approve`

```text
Usage: agentctl approvals approve [OPTIONS] --reason <REASON> <APPROVAL_ID>

Arguments:
  <APPROVAL_ID>

Options:
      --actor <ACTOR>    [default: cli-user]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --reason <REASON>
      --verbose
  -h, --help             Print help
```

## `agentctl approvals reject`

```text
Usage: agentctl approvals reject [OPTIONS] --reason <REASON> <APPROVAL_ID>

Arguments:
  <APPROVAL_ID>

Options:
      --actor <ACTOR>    [default: cli-user]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --reason <REASON>
      --verbose
  -h, --help             Print help
```

## `agentctl providers`

```text
Inspect provider capabilities or run the opt-in OpenAI smoke

Usage: agentctl providers [OPTIONS] <COMMAND>

Commands:
  inspect
  smoke-openai

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl providers inspect`

```text
Usage: agentctl providers inspect [OPTIONS] <FILE>

Arguments:
  <FILE>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl providers smoke-openai`

```text
Usage: agentctl providers smoke-openai [OPTIONS] --live

Options:
      --live             Required acknowledgement that this performs one bounded live request
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --model <MODEL>    [default: gpt-5.6]
      --verbose
  -h, --help             Print help
```

## `agentctl auth`

```text
Check configured secret references without revealing values

Usage: agentctl auth [OPTIONS] <COMMAND>

Commands:
  check

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl schema`

```text
Print or write the generated workflow JSON Schema

Usage: agentctl schema [OPTIONS]

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --write <WRITE>
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl migrate`

```text
Translate an unversioned TypeScript-era workflow into v1alpha1

Usage: agentctl migrate [OPTIONS] <FILE>

Arguments:
  <FILE>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --write <WRITE>
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl packs`

```text
Inspect and verify a local reusable pack

Usage: agentctl packs [OPTIONS] <COMMAND>

Commands:
  inspect
  verify

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl artifacts`

```text
Inspect, verify, export, or collect durable artifacts

Usage: agentctl artifacts [OPTIONS] <COMMAND>

Commands:
  list
  inspect
  verify
  export
  gc

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl artifacts list`

```text
Usage: agentctl artifacts list [OPTIONS]

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --run <RUN>
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --task <TASK>
      --verbose
  -h, --help             Print help
```

## `agentctl artifacts inspect`

```text
Usage: agentctl artifacts inspect [OPTIONS] <DIGEST>

Arguments:
  <DIGEST>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl artifacts verify`

```text
Usage: agentctl artifacts verify [OPTIONS] [DIGEST]

Arguments:
  [DIGEST]

Options:
      --all
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl artifacts export`

```text
Usage: agentctl artifacts export [OPTIONS] <DIGEST> <DESTINATION>

Arguments:
  <DIGEST>
  <DESTINATION>

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --overwrite
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl artifacts gc`

```text
Usage: agentctl artifacts gc [OPTIONS]

Options:
      --older-than-days <OLDER_THAN_DAYS>  [default: 30]
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --dry-run
      --verbose
  -h, --help                               Print help
```

## `agentctl db`

```text
Inspect the runtime database

Usage: agentctl db [OPTIONS] <COMMAND>

Commands:
  stats
  migrate
  encryption

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl db encryption`

```text
Usage: agentctl db encryption [OPTIONS] <COMMAND>

Commands:
  inventory  Inventory protected fields without exposing their values
  enable     Transactionally encrypt every identified sensitive field
  rotate     Transactionally decrypt and re-encrypt every protected field with a new key

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl db encryption inventory`

```text
Inventory protected fields without exposing their values

Usage: agentctl db encryption inventory [OPTIONS]

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl db encryption enable`

```text
Transactionally encrypt every identified sensitive field

Usage: agentctl db encryption enable [OPTIONS] --key-id <KEY_ID> --key-env <KEY_ENV>

Options:
      --key-id <KEY_ID>
      --output <OUTPUT>    [default: human] [possible values: human, json]
      --color <COLOR>      [default: auto] [possible values: auto, always, never]
      --key-env <KEY_ENV>  Environment variable containing a base64-encoded 32-byte key
      --dry-run
      --verbose
  -h, --help               Print help
```

## `agentctl db encryption rotate`

```text
Transactionally decrypt and re-encrypt every protected field with a new key

Usage: agentctl db encryption rotate [OPTIONS] --key-id <KEY_ID> --key-env <KEY_ENV>

Options:
      --key-id <KEY_ID>
      --output <OUTPUT>    [default: human] [possible values: human, json]
      --color <COLOR>      [default: auto] [possible values: auto, always, never]
      --key-env <KEY_ENV>  Environment variable containing a base64-encoded 32-byte key
      --dry-run
      --verbose
  -h, --help               Print help
```

## `agentctl memory`

```text
Read or write namespaced long-term memory

Usage: agentctl memory [OPTIONS] <COMMAND>

Commands:
  get
  put

Options:
      --db <DB>          [default: .agentctl/runtime.db]
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl gc`

```text
Garbage-collect expired memory and old terminal runs

Usage: agentctl gc [OPTIONS]

Options:
      --db <DB>                            [default: .agentctl/runtime.db]
      --output <OUTPUT>                    [default: human] [possible values: human, json]
      --color <COLOR>                      [default: auto] [possible values: auto, always, never]
      --older-than-days <OLDER_THAN_DAYS>  [default: 30]
      --verbose
  -h, --help                               Print help
```

## `agentctl completion`

```text
Generate completion for a supported shell

Usage: agentctl completion [OPTIONS] <SHELL>

Arguments:
  <SHELL>  [possible values: bash, elvish, fish, powershell, zsh]

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl version`

```text
Print the exact build version

Usage: agentctl version [OPTIONS]

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```

## `agentctl update`

```text
Explain safe update options without modifying the installation

Usage: agentctl update [OPTIONS]

Options:
      --output <OUTPUT>  [default: human] [possible values: human, json]
      --color <COLOR>    [default: auto] [possible values: auto, always, never]
      --verbose
  -h, --help             Print help
```
