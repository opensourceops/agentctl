# Variables and instruction files

Use `varsFiles` for ordinary configuration shared across workflow tasks, agent defaults, and task overrides. Use `instructionsFile` for an agent's reviewed instruction text. Both are captured configuration inputs; they grant no filesystem, process, network, tool, or approval authority.

Typed invocation inputs remain under `${{ inputs.NAME }}`. Variables are a separate `${{ vars.NAME }}` namespace. The existing `--input`, `--inputs`, and `--inputs-file` flags continue to set inputs; they do not become variable overrides.

## Precedence

Each higher row replaces lower values with the same top-level key. Files and flags within one row apply in their listed order, so the last occurrence wins.

| Low to high | Source | Scope |
| --- | --- | --- |
| 1 | `spec.varsFiles` | Ordered workflow default files. |
| 2 | `spec.vars` | Inline workflow defaults. |
| 3 | `spec.agents.NAME.varsFiles` | Ordered defaults for the selected agent. |
| 4 | `spec.agents.NAME.vars` | Inline defaults for the selected agent. |
| 5 | `spec.tasks[].varsFiles` | Ordered files for that task. |
| 6 | `spec.tasks[].vars` | Inline overrides for that task. |
| 7 | Repeated `--vars-file FILE` | Explicit invocation overrides for every task. |
| 8 | Repeated `--var KEY=JSON` | Explicit invocation overrides for every task; these win over all invocation files. |

The flag families have fixed precedence even when interleaved on the command line. Repeating `--var replicas=2 --var replicas=3` selects the JSON number `3`. Set a string with `--var 'environment="staging"'`; the value after `=` must be valid JSON.

Existing workflows without these new fields retain agent inline defaults followed by task inline overrides. Action tasks have no agent-default layer. Typed inputs keep their existing separate order: workflow input defaults, an explicit `--inputs` or `--inputs-file` object, then repeated `--input` overrides. An input becomes available to a template through `inputs.NAME`; it never silently replaces `vars.NAME`.

Sub-workflow child tasks use the same workflow, selected-agent, local-task, and invocation variable layers. A `uses: workflow:NAME` invocation keeps its existing narrow contract: pass data through typed `with` inputs rather than declaring caller task-local variables. The sub-workflow combines `with` values with its declared input defaults at its input boundary. Use explicit invocation `--var` overrides only when the override is intended to affect every task.

## Values and merge rules

A variable file is a UTF-8 YAML or JSON object with string keys. It is a data file, not a second workflow document.

| Value | When a higher source supplies the same key |
| --- | --- |
| Object | Replace the entire object; no implicit deep merge. |
| Array | Replace the entire array; do not concatenate. |
| String, number, or boolean | Replace the entire value. |
| Explicit `null` | Replace with JSON null. |
| Missing key | Keep the lower value. |
| Different JSON type | Replace at the key; templates and declared input/output schemas still enforce their own type requirements. |

Duplicate YAML keys are errors. A top-level list or scalar is invalid. Files do not import process environment variables and cannot recursively include other variable files; the top-level include directives `varsFiles`, `include`, and `includes` are rejected. A key named `policy` is ordinary data; it cannot change the workflow policy. The names `loopIndex`, `loopPrevious`, `foreachIndex`, `matrix`, and `matrixIndex` are reserved for engine bindings at every variable layer.

An ordinary object such as `{file: report.txt}` or `{env: REGION}` remains data; it is never dereferenced as a secret or environment import. Only dedicated adapter secret fields interpret secret references.

Variable values are data, not recursively evaluated expressions. A string that contains template syntax does not trigger another expansion pass after it is selected by a template. Put template expressions in the consuming task fields or instruction text.

Do not put credentials, tokens, or confidential secret material in ordinary variables or CLI flags. Use dedicated [secret references](guides/SECRET_REFERENCES.md). Variable values can enter prompts, task outputs, artifacts, and retained execution state. [Sensitive-state encryption](guides/SENSITIVE_STATE_ENCRYPTION.md) protects selected persisted fields when configured; it does not turn an ordinary variable file into a secret reference.

## A credential-free example

Save the following as `defaults.yaml`:

```yaml
environment: staging
replicas: 2
```

Save this as `workflow.yaml` in the same directory:

```yaml
apiVersion: agentctl.dev/v1
kind: Workflow
metadata:
  name: variable-precedence
spec:
  inputs:
    build: fixture-001
  varsFiles: [defaults.yaml]
  vars:
    replicas: 3
  actions:
    report:
      kind: builtin.assign
  tasks:
    - id: report
      uses: action:report
      vars:
        environment: qa
      with:
        environment: "${{ vars.environment }}"
        replicas: "${{ vars.replicas }}"
        build: "${{ inputs.build }}"
  outputs:
    report: "${{ tasks.report.output.output }}"
```

From that directory:

```text
agentctl check workflow.yaml
agentctl explain workflow.yaml --output json
agentctl run workflow.yaml --db .agentctl/variables.db --output json
agentctl run workflow.yaml --db .agentctl/variables.db --var replicas=5 --input 'build="fixture-002"' --output json
```

The first run returns a report with `environment: qa`, `replicas: 3`, and `build: fixture-001`. The second returns `replicas: 5` and `build: fixture-002`; `environment` remains `qa`. The runs use deterministic assignment and require no provider credential or network access. Inspect the returned run ID with `agentctl inspect RUN_ID --db .agentctl/variables.db --output json`. Remove this disposable example directory when its SQLite history is no longer needed.

## Instruction files

Every agent defines exactly one of `instructions` and `instructionsFile`. They are alternative sources of the same instruction text. There is no implicit concatenation or additional instruction-file syntax.

For example, an agent can declare `instructionsFile: instructions/review.md` and put this text in that file:

```text
Review build ${{ inputs.build }} for ${{ vars.environment }}.
Use only the tools visible to this agent and cite the supplied evidence.
```

Instruction templates use the same input, variable, memory, and dependency-output namespaces as inline instructions. They expand against the task context before provider dispatch. Exact templates preserve JSON types, while interpolation inside text accepts only scalar values. Missing values and invalid types fail rather than silently producing an empty instruction. The final instruction supplied to a provider must be text.

## File origins and read policy

| Declaration | Base for a relative path |
| --- | --- |
| Workflow `varsFiles` or `instructionsFile` | Directory containing the workflow file. |
| Pack agent or exported sub-workflow task | Directory containing the declaring pack manifest. |
| Inline sub-workflow task | Its enclosing workflow or pack origin. |
| Explicit `--vars-file` | The invoking process's current directory. |

Changing the invoking shell directory does not reinterpret a workflow or pack declaration. The selected workspace remains a separate policy boundary: use `--workspace` when invoking a workflow from outside the intended workspace. Container paths refer to the mounted filesystem visible to the container; mount configuration and required variable/instruction files together and keep them inside the authorized read boundary.

Paths must be nonempty literals; path templates are rejected. Source reads use the workflow read policy. They reject `..` traversal, canonical paths outside the workspace, and symlink escapes. A pack does not broaden the caller's authority. Inputs must be regular readable files, valid UTF-8, and at most 1 MiB each. Capture accepts at most 256 source-file references and 16 MiB of total source bytes. Missing files, directories or special files, invalid encoding, oversized inputs, and policy denial produce source diagnostics before provider dispatch. Include the workflow/agent/task field and source file when reporting such a failure; never paste confidential contents into an issue.

A pack manifest must declare its instruction and variable source files in its `files` map. Each entry maps a normalized portable relative path to its exact `sha256:` content digest. Declared assets must be bounded regular UTF-8 text files; `packs verify` checks every entry, including unused files, for containment, type, size, encoding, and digest. The locked manifest binds those declarations, optional signature verification binds publisher identity, and source capture checks the actual bytes against the declared digest. Update both the manifest's file digests and the reviewed pack lock after intentional content changes. See [Packs](PACKS.md).

## Capture, approvals, and recovery

Source resolution captures file bytes before compilation. Compilation and subsequent execution consume that captured configuration, so an edit after capture cannot alter the approved operation in flight. Content fingerprints bind the configuration to workflow and task compatibility checks. `check`, `plan`, `doctor`, and `explain` are separate invocations: each intentionally inspects current source files. A later `run` captures current bytes again; an earlier plan is not an authorization token for unrelated later bytes.

| Command | Source and effect behavior |
| --- | --- |
| `run` | Capture current configuration, compile, and apply policy and approval checks to fresh work. |
| `resume` | Continue the recorded run using its captured configuration and durable effect state. |
| `replay` | Reconstruct recorded execution without rereading mutable instruction/variable inputs or making fresh provider/effect requests. |
| `retry` | Require an identical captured workflow for terminal retry; changed source requires repair. |
| `repair --plan` | Compare the new captured target with the source run, explain reusable/invalidated boundaries, and dispatch no work. |
| `repair` | Reuse only compatible successful boundaries and recheck authority for fresh work. |
| `fork` | Create an intentional fresh execution from recorded configuration; fresh effects remain governed. |

Changed instruction text or a winning variable value invalidates affected reuse and approval decisions. Unaffected boundaries can remain reusable when dependency, schema, artifact, memory, and effect-safety checks also pass. A source edit never makes an uncertain non-idempotent effect safe to repeat. Reconcile the remote result before recovery when required.

Recorded replay needs no provider key. An encrypted state database still needs its configured state-encryption key; provider-keyless replay does not bypass database protection.

Inline-only workflows omit unused capture fields and preserve existing digest compatibility. Older file-backed histories did not retain the complete new source snapshot. Loading the same files now can therefore change the workflow digest and require `repair` instead of identical-workflow `retry`. Repair can compare a previously successful recorded instruction-read digest with the new captured bytes. A file that was never captured or successfully recorded in the old history cannot be reconstructed after it disappears. The no-reread guarantees above apply to runs created with captured sources.

## Explain and preflight

`agentctl explain workflow.yaml --output json` shows the workflow name, compiled plan digest, source layers, and effective winning variable origins for task scopes without printing variable values or instruction text. Use the same `--vars-file` and `--var` overrides intended for execution. Source names and the plan digest support review, but an origin report is not a policy grant or a credential validation result.

`agentctl doctor workflow.yaml --output json` adds non-dispatching prerequisite checks. A missing provider credential fails readiness without showing the value. A process secret that would require running an external command remains unverified and makes readiness false. For container actions, the doctor checks the engine and pinned local image with bounded inspection; it does not pull an image or run a container. These checks establish available prerequisites, not successful workflow execution. A failed or unverified prerequisite uses exit code `6`; inspect the structured checks before scheduling a live run.

See [Workflow DSL](DSL.md), the [YAML reference](reference/YAML.md), [scheduled operation](OPERATIONS.md), and [selective repair](guides/repair-a-failed-workflow.md).
