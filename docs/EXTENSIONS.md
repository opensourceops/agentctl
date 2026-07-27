# Extensions

agentctl has no in-process native plugin ABI. Loading third-party dynamic
libraries into the runtime would weaken Rust memory safety, make cancellation
platform-specific, and bypass the effect and policy boundary.

The supported extension contracts are:

- reviewed declarative packs for reusable definitions;
- MCP for remote tools;
- `extension.process` for a local executable with a versioned bounded protocol.

## Process protocol

An `extension.process` action declares direct argv, protocol version,
idempotency, JSON Schemas, capabilities, environment secret references,
timeout, and output limits:

```yaml
actions:
  transform:
    kind: extension.process
    command: ./bin/transform-extension
    args: [serve]
    protocolVersion: agentctl.dev/process-extension/v1
    idempotency: keyed
    capabilities: [document.transform]
    timeoutSeconds: 10
    stdoutLimitBytes: 1048576
    stderrLimitBytes: 1048576
    combinedOutputLimitBytes: 2097152
    inputSchema:
      type: object
      required: [value]
      properties:
        value: { type: string }
      additionalProperties: false
    outputSchema:
      type: object
      required: [value]
      properties:
        value: { type: string }
      additionalProperties: false
```

Before invocation, agentctl executes the declared command plus
`--agentctl-handshake`. The executable writes one JSON object:

```json
{
  "protocolVersion": "agentctl.dev/process-extension/v1",
  "name": "example-transform",
  "inputSchema": {},
  "outputSchema": {},
  "capabilities": ["document.transform"]
}
```

The version, schemas, and ordered capability list must exactly match the action.
A mismatch fails before invocation.

agentctl then executes a fresh process plus `--agentctl-invoke` and writes:

```json
{
  "protocolVersion": "agentctl.dev/process-extension/v1",
  "effectId": "stable-effect-identity",
  "input": {}
}
```

The executable returns:

```json
{
  "protocolVersion": "agentctl.dev/process-extension/v1",
  "effectId": "stable-effect-identity",
  "output": {}
}
```

The response identity and output schema are mandatory. Invocation transport,
timeout, cancellation, crash, output overflow, malformed response, or contract
failure is durable uncertainty because the extension may have acted before the
failure became visible. Reconciliation, retry, repair, replay, and fork retain
the ordinary effect-ledger semantics.

## Security boundary

The executable basename must pass `policy.processAllowlist`. Arguments are
direct, without a shell. The environment is cleared and only explicitly
allowlisted secret references are resolved. Known secret values are redacted
before JSON is parsed or persisted. Stdout, stderr, combined output, time, and
cancellation are bounded. Unix descendants are terminated as a process group;
Windows descendants use process-tree termination.

A pack containing `builtin.shell.exec` or `extension.process` must satisfy pack
trust policy before any action is loaded. An unsigned process pack requires the
explicit `allowUnsignedProcess` review acknowledgement.

These controls are not an operating-system sandbox. A permitted executable
runs with the agentctl process identity and can access whatever that identity
can access. Use a restricted container, VM, platform sandbox, and least
privilege credentials for hostile or unreviewed code.
