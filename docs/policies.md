# Policies and approvals

Policy is evaluated by the runtime, never by a model. A policy defines a canonical workspace root, writable roots, allowed environment names, network host patterns, process basenames, providers, tool allow/deny lists, approval mode, and non-interactive behavior.

Read paths must canonicalize under the workspace. Write paths canonicalize the nearest existing parent and must remain under a writable root. Parent traversal and symlink escape fail. Network rules match an exact hostname or `*.suffix` subdomains; suffix lookalikes and the wildcard apex do not match. HTTP redirects are disabled. Process allowlisting checks the executable basename and then launches direct argv with a cleared environment.

Tool visibility, tool/capability authorization, resource checks, effect risk, and approval are distinct decisions. `never`, `mutations`, `high_risk`, and `always` are available approval modes. A tool may say `never`, `policy`, or `always`. The default non-interactive behavior is a durable pause and exit code `3`; explicit `deny_approval` and `fail` modes fail closed. Non-interactive execution never prompts or auto-approves.

An approval stores the run/trace/task/agent, tool, capability, risk, redacted input, expected effect, reason, and resolution actor/reason. The associated task waits durably. Use `approvals list`, `approve`, or `reject`, then `resume`. Resolution and effect status are auditable.

Provider, MCP, A2A, filesystem, process, and environment allowlists are necessary controls, not a containment boundary. Run untrusted executors inside an external OS/container sandbox.
